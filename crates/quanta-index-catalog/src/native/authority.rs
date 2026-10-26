//! Native incremental catalog authority. These tables share the existing
//! catalog connection, durability settings and initialization transaction.
//!
//! The operation key and the source frontier occupy separate namespaces. A
//! committed view, its work, custody transfer, and original operation receipt
//! are one `SQLite` transaction. This module never scans a corpus on admission
//! or publication.

use std::{path::Path, time::Duration};

mod boot_manifest;
mod read_only;
mod read_settlement;
pub use read_only::{
    NativeBootCatalogCandidate, NativeBootCatalogSnapshot, verify_native_boot_candidate,
    verify_native_boot_catalog,
};
mod bootstrap;
mod capture_format;
mod fence_counter;
mod lifetime;
mod managed_install;
mod object_install;
mod owned_work;
mod reclaim;
mod retention;
mod source_authority;
mod terminal_custody;
mod terminal_work;

use quanta_index_core::CoreError;
use quanta_index_core::index::ports::{
    CatalogGenesisPort, CatalogPublicationPort, CatalogReclaimPort, ImmutableObjectPort,
    InputStagingPort, ManagedObjectAuthority, ManagedObjectInstallPort, ObjectCustodyPort,
    OperationInspectAccess, OperationJournalPort, WorkActionCursor, WorkClaimPort,
};
use quanta_index_core::index::{
    AdmissionOutcome, AttemptId, AttemptRef, BootstrapRef, CatalogView, ImmutableOperation,
    IndexError, IngressRef, OWNED_WORK_PRODUCER, ObjectInstallScope, ObjectNamespaceBinding,
    ObjectRef, OperationKey, OperationStatus, OwnedWorkAssignment, OwnerMemberBinding,
    PreparedPublication, ReclaimCandidate, ReclaimDrainPage, ReclaimPermit, ReclaimTarget,
    ReclaimWorkPage, Refusal, RegisteredObject, RetiringRef, SourceEvent, SourceOperationMode,
    TicketId, UpdateKind, UpdateRequest, ViewRef, WorkAction, WorkActionDigest, WorkBudget,
    WorkClaim, WorkIntent, WorkScope, WorkStatus, WorkUsage, WorkspaceKey,
};
use rusqlite::{Connection, OptionalExtension as _, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::connection::{SqliteCatalog, engine_error};

pub use boot_manifest::NativeBootManifest;

/// Only native tables are added here; no competing `SQLite` file or head.
const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS native_capture_format_v1 (
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
) WITHOUT ROWID;
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
CREATE TABLE IF NOT EXISTS native_exact_history_floor_v1 (
    workspace BLOB NOT NULL,
    epoch INTEGER NOT NULL CHECK(epoch >= 0),
    retain_from_sequence INTEGER NOT NULL CHECK(retain_from_sequence >= 0),
    row_digest BLOB NOT NULL,
    PRIMARY KEY(workspace, epoch),
    FOREIGN KEY(workspace) REFERENCES native_workspace_v1(workspace)
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
CREATE TABLE IF NOT EXISTS native_terminal_custody_v1 (
    operation BLOB PRIMARY KEY NOT NULL,
    ready_sequence INTEGER UNIQUE NOT NULL CHECK(ready_sequence > 0),
    workspace BLOB NOT NULL,
    epoch INTEGER NOT NULL CHECK(epoch >= 0),
    row_digest BLOB NOT NULL,
    FOREIGN KEY(operation) REFERENCES native_operation_v1(operation)
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
CREATE TABLE IF NOT EXISTS native_gc_namespace_v1 (
    singleton INTEGER PRIMARY KEY NOT NULL CHECK(singleton = 1),
    payload BLOB NOT NULL,
    row_digest BLOB NOT NULL
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_gc_object_v1 (
    object BLOB PRIMARY KEY NOT NULL,
    generation INTEGER NOT NULL CHECK(generation > 0),
    kind INTEGER NOT NULL CHECK(kind IN (0,1)),
    phase INTEGER NOT NULL CHECK(phase IN (0,1,2,3,4)),
    direct_children INTEGER NOT NULL CHECK(direct_children BETWEEN 0 AND 4096),
    drained_children INTEGER NOT NULL CHECK(drained_children BETWEEN 0 AND direct_children),
    target BLOB,
    row_digest BLOB NOT NULL
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_gc_queue_v1 (
    object BLOB PRIMARY KEY NOT NULL,
    row_digest BLOB NOT NULL,
    FOREIGN KEY(object) REFERENCES native_gc_object_v1(object)
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
CREATE TABLE IF NOT EXISTS native_object_install_v1 (
    object BLOB PRIMARY KEY NOT NULL,
    payload BLOB NOT NULL,
    row_digest BLOB NOT NULL
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_object_install_edge_v1 (
    parent BLOB NOT NULL,
    child BLOB NOT NULL,
    row_digest BLOB NOT NULL,
    PRIMARY KEY(parent, child),
    FOREIGN KEY(parent) REFERENCES native_object_install_v1(object)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_bootstrap_v1 (
    workspace BLOB PRIMARY KEY NOT NULL,
    epoch INTEGER NOT NULL CHECK(epoch >= 0),
    fence INTEGER NOT NULL CHECK(fence > 0),
    profile BLOB NOT NULL,
    payload BLOB NOT NULL,
    row_digest BLOB NOT NULL
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_bootstrap_object_v1 (
    workspace BLOB PRIMARY KEY NOT NULL,
    object BLOB NOT NULL,
    payload BLOB NOT NULL,
    confirmed INTEGER NOT NULL CHECK(confirmed IN (0,1)),
    row_digest BLOB NOT NULL,
    FOREIGN KEY(workspace) REFERENCES native_bootstrap_v1(workspace)
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
    session_id INTEGER NOT NULL CHECK(session_id > 0),
    session_fence INTEGER NOT NULL CHECK(session_fence > 0),
    fence INTEGER NOT NULL CHECK(fence > 0),
    expires_at_ms INTEGER NOT NULL CHECK(expires_at_ms > 0),
    workspace BLOB NOT NULL,
    epoch INTEGER NOT NULL,
    sequence INTEGER NOT NULL,
    row_digest BLOB,
    FOREIGN KEY(workspace, epoch, sequence) REFERENCES native_view_v1(workspace, epoch, sequence),
    FOREIGN KEY(session_id) REFERENCES native_read_session_v1(id)
);
CREATE TABLE IF NOT EXISTS native_read_session_v1 (
    id INTEGER PRIMARY KEY NOT NULL CHECK(id > 0),
    fence INTEGER NOT NULL CHECK(fence > 0),
    device BLOB NOT NULL,
    inode BLOB NOT NULL,
    row_digest BLOB NOT NULL
);
CREATE TABLE IF NOT EXISTS native_read_session_open_v1 (
    id INTEGER PRIMARY KEY NOT NULL CHECK(id > 0),
    fence INTEGER NOT NULL CHECK(fence > 0),
    row_digest BLOB NOT NULL
);
CREATE TABLE IF NOT EXISTS native_read_session_retire_v1 (
    id INTEGER PRIMARY KEY NOT NULL CHECK(id > 0),
    device BLOB NOT NULL,
    inode BLOB NOT NULL,
    row_digest BLOB NOT NULL
);
CREATE TABLE IF NOT EXISTS native_work_v1 (
    work BLOB PRIMARY KEY NOT NULL,
    operation BLOB NOT NULL,
    workspace BLOB NOT NULL,
    epoch INTEGER NOT NULL,
    sequence INTEGER NOT NULL,
    status INTEGER NOT NULL CHECK(status IN (0,1,2,3,4)),
    owner_mode INTEGER NOT NULL CHECK(owner_mode IN (0,1,2)),
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
CREATE TABLE IF NOT EXISTS native_owned_work_v1 (
    work BLOB PRIMARY KEY NOT NULL,
    workspace BLOB NOT NULL,
    epoch INTEGER NOT NULL,
    due_ms INTEGER NOT NULL CHECK(due_ms >= 0),
    active INTEGER NOT NULL CHECK(active IN (0,1)),
    payload BLOB NOT NULL,
    row_digest BLOB NOT NULL,
    FOREIGN KEY(work) REFERENCES native_work_v1(work)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_owned_request_v1 (
    operation BLOB PRIMARY KEY NOT NULL,
    work BLOB NOT NULL,
    payload BLOB NOT NULL,
    row_digest BLOB NOT NULL
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
CREATE TABLE IF NOT EXISTS native_source_authority_v1 (
    workspace BLOB NOT NULL,
    workspace_epoch INTEGER NOT NULL CHECK(workspace_epoch >= 0),
    producer BLOB NOT NULL,
    source_epoch INTEGER NOT NULL CHECK(source_epoch >= 0),
    row_digest BLOB NOT NULL,
    PRIMARY KEY(workspace, workspace_epoch, producer)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS native_operation_workspace_idx_v1
    ON native_operation_v1(workspace, epoch);
CREATE INDEX IF NOT EXISTS native_terminal_custody_scope_idx_v1
    ON native_terminal_custody_v1(workspace, epoch, ready_sequence);
CREATE INDEX IF NOT EXISTS native_event_claim_operation_idx_v1
    ON native_event_claim_v1(operation);
CREATE INDEX IF NOT EXISTS native_pin_view_idx_v1
    ON native_pin_v1(workspace, epoch, sequence);
CREATE INDEX IF NOT EXISTS native_pin_session_idx_v1
    ON native_pin_v1(session_id, session_fence, ticket);
CREATE INDEX IF NOT EXISTS native_work_view_idx_v1
    ON native_work_v1(workspace, epoch, status, lease_until_ms, work);
CREATE INDEX IF NOT EXISTS native_work_created_view_idx_v1
    ON native_work_v1(workspace, epoch, sequence, work);
CREATE INDEX IF NOT EXISTS native_owned_request_work_idx_v1
    ON native_owned_request_v1(work, operation);
CREATE INDEX IF NOT EXISTS native_work_pending_idx_v1
    ON native_work_v1(workspace, epoch, status, ready_since_ms, work);
CREATE INDEX IF NOT EXISTS native_owned_open_idx_v1
    ON native_work_v1(workspace, epoch, owner_mode, status, ready_since_ms, work);
CREATE INDEX IF NOT EXISTS native_owned_claim_idx_v1
    ON native_work_v1(workspace, epoch, owner_mode, status, lease_until_ms, work);
CREATE INDEX IF NOT EXISTS native_owned_due_idx_v1
    ON native_owned_work_v1(workspace, epoch, due_ms, work);
CREATE INDEX IF NOT EXISTS native_view_root_object_idx_v1 ON native_view_root_v1(object,workspace,epoch,sequence);
CREATE INDEX IF NOT EXISTS native_operation_object_object_idx_v1 ON native_operation_object_v1(object,operation);
CREATE INDEX IF NOT EXISTS native_ingress_intent_object_idx_v1 ON native_ingress_intent_v1(object,operation);
CREATE INDEX IF NOT EXISTS native_ingress_object_object_idx_v1 ON native_ingress_object_v1(object,operation);
CREATE INDEX IF NOT EXISTS native_attempt_object_object_idx_v1 ON native_attempt_object_v1(object,operation,fence);
CREATE INDEX IF NOT EXISTS native_object_intent_object_idx_v1 ON native_object_intent_v1(object,operation,fence);
CREATE INDEX IF NOT EXISTS native_work_object_object_idx_v1 ON native_work_object_v1(object,work);
CREATE INDEX IF NOT EXISTS native_bootstrap_object_object_idx_v1 ON native_bootstrap_object_v1(object,workspace);
CREATE INDEX IF NOT EXISTS native_bootstrap_profile_idx_v1 ON native_bootstrap_v1(profile,workspace);
CREATE INDEX IF NOT EXISTS native_object_edge_child_idx_v1 ON native_object_edge_v1(child,parent);
CREATE INDEX IF NOT EXISTS native_object_install_edge_child_idx_v1
    ON native_object_install_edge_v1(child, parent);
";

const READY_PENDING_QUERY: &str = "SELECT work,length(payload),ready_since_ms FROM native_work_v1
    WHERE workspace=?1 AND epoch=?2 AND owner_mode=0 AND status=0 ORDER BY ready_since_ms,work LIMIT ?3";
const READY_EXPIRED_QUERY: &str = "SELECT work,length(payload),lease_until_ms FROM native_work_v1
    WHERE workspace=?1 AND epoch=?2 AND owner_mode=1 AND status=1 AND lease_until_ms<=?3
    ORDER BY lease_until_ms,work LIMIT ?4";

const SCHEMA_OBJECTS: &[(&str, &str, &str)] = &[
    ("table", "native_capture_format_v1", "native capture format"),
    ("table", "native_workspace_v1", "native workspace"),
    ("table", "native_view_v1", "native view"),
    (
        "table",
        "native_exact_history_floor_v1",
        "native exact history floor",
    ),
    ("table", "native_operation_v1", "native operation"),
    (
        "table",
        "native_terminal_custody_v1",
        "native terminal custody ready set",
    ),
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
    (
        "table",
        "native_gc_namespace_v1",
        "native GC namespace binding",
    ),
    ("table", "native_gc_object_v1", "native GC object lifecycle"),
    ("table", "native_gc_queue_v1", "native GC candidate queue"),
    ("table", "native_object_v1", "native object"),
    ("table", "native_object_edge_v1", "native object edge"),
    (
        "table",
        "native_object_install_v1",
        "native provisional object",
    ),
    (
        "table",
        "native_object_install_edge_v1",
        "native provisional object child",
    ),
    ("table", "native_bootstrap_v1", "native bootstrap authority"),
    (
        "table",
        "native_bootstrap_object_v1",
        "native bootstrap object custody",
    ),
    ("table", "native_attempt_object_v1", "native attempt object"),
    ("table", "native_object_intent_v1", "native object intent"),
    ("table", "native_view_root_v1", "native view root"),
    ("table", "native_pin_v1", "native pin"),
    ("table", "native_read_session_v1", "native read session"),
    (
        "table",
        "native_read_session_open_v1",
        "native read session opening intent",
    ),
    (
        "table",
        "native_read_session_retire_v1",
        "native read session retirement",
    ),
    ("table", "native_work_v1", "native work"),
    ("table", "native_work_object_v1", "native work object"),
    ("table", "native_owned_work_v1", "native owned work"),
    ("table", "native_owned_request_v1", "native owned request"),
    ("table", "native_work_clock_v1", "native work clock"),
    ("table", "native_frontier_v1", "native frontier"),
    (
        "table",
        "native_source_authority_v1",
        "native producer source epoch authority",
    ),
    (
        "index",
        "native_operation_workspace_idx_v1",
        "native operation workspace index",
    ),
    (
        "index",
        "native_terminal_custody_scope_idx_v1",
        "native terminal custody scope index",
    ),
    (
        "index",
        "native_event_claim_operation_idx_v1",
        "native event claim operation index",
    ),
    ("index", "native_pin_view_idx_v1", "native pin view index"),
    (
        "index",
        "native_pin_session_idx_v1",
        "native pin session index",
    ),
    ("index", "native_work_view_idx_v1", "native work view index"),
    (
        "index",
        "native_work_created_view_idx_v1",
        "native work created view index",
    ),
    (
        "index",
        "native_owned_request_work_idx_v1",
        "native owned request work index",
    ),
    (
        "index",
        "native_work_pending_idx_v1",
        "native work pending index",
    ),
    (
        "index",
        "native_owned_open_idx_v1",
        "native owned open index",
    ),
    (
        "index",
        "native_owned_claim_idx_v1",
        "native owned claim index",
    ),
    ("index", "native_owned_due_idx_v1", "native owned due index"),
    (
        "index",
        "native_view_root_object_idx_v1",
        "native view root object index",
    ),
    (
        "index",
        "native_operation_object_object_idx_v1",
        "native operation object reverse index",
    ),
    (
        "index",
        "native_ingress_intent_object_idx_v1",
        "native ingress intent reverse index",
    ),
    (
        "index",
        "native_ingress_object_object_idx_v1",
        "native ingress object reverse index",
    ),
    (
        "index",
        "native_attempt_object_object_idx_v1",
        "native attempt object reverse index",
    ),
    (
        "index",
        "native_object_intent_object_idx_v1",
        "native object intent reverse index",
    ),
    (
        "index",
        "native_work_object_object_idx_v1",
        "native work object reverse index",
    ),
    (
        "index",
        "native_bootstrap_object_object_idx_v1",
        "native bootstrap object reverse index",
    ),
    (
        "index",
        "native_bootstrap_profile_idx_v1",
        "native bootstrap profile reverse index",
    ),
    (
        "index",
        "native_object_edge_child_idx_v1",
        "native registered child reverse index",
    ),
    (
        "index",
        "native_object_install_edge_child_idx_v1",
        "native provisional child index",
    ),
];

pub(crate) use capture_format::preflight_capture_format;

pub(crate) fn initialize(
    connection: &Connection,
    path: &Path,
    native_first_birth: bool,
) -> Result<(), CoreError> {
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
    capture_format::seed_and_verify(connection, path, native_first_birth)?;
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
            "SELECT next,CASE WHEN typeof(row_digest)='blob' AND length(row_digest)=32 THEN row_digest END FROM native_counter_v1 WHERE name=?1",
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
    if crate::io_admission::is_capacity_refusal(error) {
        IndexError::ResourceLimited
    } else {
        IndexError::Storage(error.to_string())
    }
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

/// Drop the checked-out statement before a nested validator reuses its SQL.
fn cached_query_row<T, P, F>(
    connection: &Connection,
    query: &str,
    parameters: P,
    read: F,
) -> rusqlite::Result<T>
where
    P: rusqlite::Params,
    F: FnOnce(&rusqlite::Row<'_>) -> rusqlite::Result<T>,
{
    connection
        .prepare_cached(query)?
        .query_row(parameters, read)
}

/// Return the checked-out statement before another operation reuses its SQL.
fn cached_execute<P: rusqlite::Params>(
    connection: &Connection,
    statement: &str,
    parameters: P,
) -> rusqlite::Result<usize> {
    connection.prepare_cached(statement)?.execute(parameters)
}

fn require_object(connection: &Connection, reference: ObjectRef) -> Result<(), IndexError> {
    let registration = object_install::read_metadata(
        connection,
        reference,
        object_install::MetadataLedger::Registered,
        || IndexError::CorruptObject("registered object metadata shape mismatch".into()),
        |_bytes| Ok(()),
    )?
    .ok_or_else(|| {
        IndexError::CorruptObject("referenced root or child is not registered".into())
    })?;
    require_registered_edges(connection, reference, &registration)
}

/// Complete validation using metadata checked by the current caller.
///
/// Shape, checksum and decoding must already be checked. This reads the full
/// edge ledger on every call and retains no evidence beyond this operation.
fn require_registered_edges(
    connection: &Connection,
    reference: ObjectRef,
    registration: &RegisteredObject,
) -> Result<(), IndexError> {
    if registration.reference != reference {
        return Err(IndexError::CorruptObject(
            "object key does not match registration".into(),
        ));
    }
    if registration.children.len() > object_install::MAX_CHILDREN
        || registration.bytes == 0
        || registration.bytes > object_install::MAX_OBJECT_BYTES
    {
        return Err(IndexError::CorruptObject(
            "registered object exceeds canonical bounds".into(),
        ));
    }
    object_install::require_edge_ledger(
        connection,
        reference,
        &registration.children,
        object_install::EdgeLedger::Registered,
    )
}

fn canonical_object(object: &RegisteredObject) -> Result<RegisteredObject, IndexError> {
    if object.bytes == 0
        || object.bytes > object_install::MAX_OBJECT_BYTES
        || object.children.len() > object_install::MAX_CHILDREN
    {
        return Err(IndexError::ResourceLimited);
    }
    if object.children.contains(&object.reference) {
        return Err(IndexError::InvalidInput("self object edge".into()));
    }
    Ok(object.clone())
}

/// Insert canonical metadata after this transaction has authenticated its children.
fn register_prepared_object(
    connection: &Connection,
    canonical: &RegisteredObject,
) -> Result<(), IndexError> {
    if !object_install::registered_matches(connection, canonical)? {
        let payload = encode(canonical)?;
        let key = object_key(canonical.reference);
        let inserted = cached_execute(
            connection,
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
            let inserted = cached_execute(
                connection,
                "INSERT INTO native_object_edge_v1(parent,child,row_digest) VALUES (?1,?2,?3)",
                params![
                    object_key(canonical.reference),
                    object_key(*child),
                    fields_digest(
                        b"qidx.native.edge.v1",
                        &(object_key(canonical.reference), object_key(*child))
                    )?
                ],
            )
            .map_err(|error| sql(&error))?;
            exactly_one(inserted, "register object edge")?;
        }
    }
    Ok(())
}

fn register(connection: &Connection, objects: &[RegisteredObject]) -> Result<(), IndexError> {
    for object in objects {
        let canonical = object_install::checked_object(object)?;
        for child in &canonical.children {
            require_object(connection, *child)?;
        }
        register_prepared_object(connection, &canonical)?;
    }
    Ok(())
}

fn register_confirmed_using(
    connection: &Connection,
    objects: &[RegisteredObject],
    register_batch: impl FnOnce(&Connection, &[RegisteredObject]) -> Result<(), IndexError>,
) -> Result<(), IndexError> {
    if connection.is_autocommit() {
        return Err(IndexError::BackendViolation(
            "object confirmation requires a transaction".into(),
        ));
    }
    // Validate the complete exact batch before any registration or custody transfer.
    for object in objects {
        object_install::require_confirmable(connection, object)?;
    }
    register_batch(connection, objects)?;
    for object in objects {
        object_install::transfer_preflighted(connection, object)?;
    }
    Ok(())
}

fn register_confirmed(
    connection: &Connection,
    objects: &[RegisteredObject],
) -> Result<(), IndexError> {
    register_confirmed_using(connection, objects, register)
}

/// Registrations authenticated by the current transaction.
///
/// No callback or registration mutation intervenes before custody transfer.
/// Private construction requires the complete unique-input child preflight.
struct RegisteredBatchInTransaction<'tx, 'conn> {
    transaction: &'tx rusqlite::Transaction<'conn>,
    objects: &'tx [RegisteredObject],
    provisional_at_preflight: [bool; managed_install::MAX_OBJECTS],
}

impl<'tx, 'conn> RegisteredBatchInTransaction<'tx, 'conn> {
    fn register(
        preflight: managed_install::PreflightedRegistrationChildren<'tx, 'conn>,
    ) -> Result<Self, IndexError> {
        if !preflight.is_unique() {
            return Err(IndexError::BackendViolation(
                "registration proof requires unique input identities".into(),
            ));
        }
        let (transaction, objects) = preflight.into_parts();
        if objects.len() > managed_install::MAX_OBJECTS {
            return Err(IndexError::ResourceLimited);
        }
        let mut provisional = [false; managed_install::MAX_OBJECTS];
        // Finish the entire provisional/registered payload and edge preflight
        // before any registration or transfer, including late corrupt input.
        for (pending, object) in provisional.iter_mut().zip(objects) {
            let state = object_install::confirmation_state(transaction, object)?;
            *pending = matches!(state, object_install::ConfirmationState::Provisional);
        }
        for (pending, object) in provisional.iter().zip(objects) {
            if *pending {
                let canonical = object_install::checked_object(object)?;
                register_prepared_object(transaction, &canonical)?;
            }
            // A replay's permanent registration was already fully checked in
            // this transaction. Unique parent keys and these fixed writes
            // cannot change another input's permanent payload or edge ledger.
        }
        Ok(Self {
            transaction,
            objects,
            provisional_at_preflight: provisional,
        })
    }

    fn parts(
        &self,
    ) -> (
        &'tx rusqlite::Transaction<'conn>,
        &'tx [RegisteredObject],
        &[bool; managed_install::MAX_OBJECTS],
    ) {
        (
            self.transaction,
            self.objects,
            &self.provisional_at_preflight,
        )
    }

    fn confirmed_ingress(
        &self,
        key: OperationKey,
        object: &RegisteredObject,
    ) -> Result<bool, IndexError> {
        if !self.objects.iter().any(|input| std::ptr::eq(input, object)) {
            return Err(IndexError::BackendViolation(
                "ingress receipt object is outside its registration proof".into(),
            ));
        }
        confirmed_ingress_membership(self.transaction, key, object.reference)
    }
}

fn register_confirmed_preflighted_children<'tx, 'conn>(
    preflight: managed_install::PreflightedRegistrationChildren<'tx, 'conn>,
) -> Result<Option<RegisteredBatchInTransaction<'tx, 'conn>>, IndexError> {
    if preflight.is_unique() {
        let registered = RegisteredBatchInTransaction::register(preflight)?;
        return object_install::transfer_registered_batch(registered).map(Some);
    }
    // Only managed confirmation creates this proof after its charged child checks.
    // It borrows the actual transaction and exact immutable batch until this
    // immediate consumption; no authentication result is cached across calls.
    let (connection, objects) = preflight.into_parts();
    register_confirmed_using(connection, objects, |connection, objects| {
        for object in objects {
            let canonical = object_install::checked_object(object)?;
            register_prepared_object(connection, &canonical)?;
        }
        Ok(())
    })?;
    Ok(None)
}

fn reserve(
    connection: &rusqlite::Transaction<'_>,
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
        reserve_object_intent(connection, attempt, &canonical)?;
        if !earlier_in_batch.insert(object.reference) {
            return Err(IndexError::InvalidInput(
                "duplicate object in reservation batch".into(),
            ));
        }
    }
    Ok(())
}

/// Mutation primitive after the caller's child-first validation.
///
/// Managed callers finish that preflight under this transaction before writing.
/// This does not validate external children or publish registered objects.
fn reserve_object_intent(
    connection: &rusqlite::Transaction<'_>,
    attempt: AttemptRef,
    canonical: &RegisteredObject,
) -> Result<(), IndexError> {
    object_install::reserve(connection, canonical)?;
    let payload = encode(canonical)?;
    if !try_insert_object_intent(connection, attempt, canonical, &payload)? {
        require_reserved_intent(connection, attempt, canonical, &payload)?;
    }
    Ok(())
}

fn try_insert_object_intent(
    connection: &Connection,
    attempt: AttemptRef,
    canonical: &RegisteredObject,
    payload: &[u8],
) -> Result<bool, IndexError> {
    let changed = cached_execute(
        connection,
        "INSERT INTO native_object_intent_v1(operation,fence,object,payload,row_digest)
         VALUES (?1,?2,?3,?4,?5) ON CONFLICT(operation,fence,object) DO NOTHING",
        params![
            operation_key(attempt.operation)?,
            int(attempt.fence)?,
            object_key(canonical.reference),
            payload,
            digest(b"qidx.native.intent.v1", payload).to_vec()
        ],
    )
    .map_err(|error| sql(&error))?;
    at_most_one(changed, "reserve object")?;
    Ok(changed == 1)
}

// The SQL guards keep malformed variable-size fields out of SQLite result BLOBs.
fn allocation_intent_columns(row: &rusqlite::Row<'_>) -> rusqlite::Result<(Vec<u8>, Vec<u8>)> {
    let payload: Option<Vec<u8>> = row.get(0)?;
    let checksum: Option<Vec<u8>> = row.get(1)?;
    match (payload, checksum) {
        (Some(payload), Some(checksum)) => Ok((payload, checksum)),
        _ => Err(rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Blob,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "native allocation intent shape invalid",
            )),
        )),
    }
}

fn require_reserved_intent(
    connection: &Connection,
    attempt: AttemptRef,
    canonical: &RegisteredObject,
    payload: &[u8],
) -> Result<(), IndexError> {
    let prior: Option<(Vec<u8>, Vec<u8>)> = cached_query_row(
        connection,
        "SELECT CASE WHEN typeof(payload)='blob' AND length(payload) BETWEEN 1 AND ?4 THEN payload END,
                CASE WHEN typeof(row_digest)='blob' AND length(row_digest)=32 THEN row_digest END
         FROM native_object_intent_v1 WHERE operation=?1 AND fence=?2 AND object=?3",
        params![
            operation_key(attempt.operation)?,
            int(attempt.fence)?,
            object_key(canonical.reference),
            object_install::MAX_METADATA_BYTES_SQL
        ],
        allocation_intent_columns,
    )
    .optional()
    .map_err(|error| sql(&error))?;
    let Some((prior, checksum)) = prior else {
        return Err(IndexError::Storage("reserved object intent missing".into()));
    };
    check_digest(b"qidx.native.intent.v1", &prior, &checksum)?;
    if prior != payload {
        return Err(IndexError::CorruptObject(
            "reserved object metadata changed".into(),
        ));
    }
    Ok(())
}

fn try_insert_ingress_intent(
    connection: &Connection,
    operation: OperationKey,
    object: &RegisteredObject,
    payload: &[u8],
) -> Result<bool, IndexError> {
    let changed = cached_execute(
        connection,
        "INSERT INTO native_ingress_intent_v1(operation,object,payload,row_digest)
         VALUES (?1,?2,?3,?4) ON CONFLICT(operation,object) DO NOTHING",
        params![
            operation_key(operation)?,
            object_key(object.reference),
            payload,
            digest(b"qidx.native.ingressintent.v1", payload).to_vec()
        ],
    )
    .map_err(|error| sql(&error))?;
    at_most_one(changed, "reserve ingress object")?;
    Ok(changed == 1)
}

fn require_reserved_ingress_intent(
    connection: &Connection,
    operation: OperationKey,
    object: &RegisteredObject,
    payload: &[u8],
) -> Result<(), IndexError> {
    let prior = ingress_intent(connection, operation, object)?
        .ok_or_else(|| IndexError::Storage("reserved ingress intent missing".into()))?;
    if prior != payload {
        return Err(IndexError::CorruptObject(
            "ingress allocation metadata changed".into(),
        ));
    }
    Ok(())
}

fn require_intent(
    connection: &Connection,
    attempt: AttemptRef,
    object: &RegisteredObject,
) -> Result<(), IndexError> {
    require_intent_with_attempt_key(connection, attempt, object, &mut None)
}

// The key is immutable within the caller's locked transaction. Each object
// still receives its own canonical-payload and durable-intent validation.
fn require_intent_with_attempt_key(
    connection: &Connection,
    attempt: AttemptRef,
    object: &RegisteredObject,
    attempt_key: &mut Option<(Vec<u8>, i64)>,
) -> Result<(), IndexError> {
    let canonical = canonical_object(object)?;
    let payload = encode(&canonical)?;
    if attempt_key.is_none() {
        *attempt_key = Some((operation_key(attempt.operation)?, int(attempt.fence)?));
    }
    let (operation, fence) = attempt_key
        .as_ref()
        .ok_or_else(|| IndexError::BackendViolation("object intent attempt key missing".into()))?;
    let prior: Option<(Vec<u8>, Vec<u8>)> = cached_query_row(
        connection,
        "SELECT CASE WHEN typeof(payload)='blob' AND length(payload) BETWEEN 1 AND ?4 THEN payload END,
                CASE WHEN typeof(row_digest)='blob' AND length(row_digest)=32 THEN row_digest END
         FROM native_object_intent_v1 WHERE operation=?1 AND fence=?2 AND object=?3",
        params![
            operation,
            fence,
            object_key(object.reference),
            object_install::MAX_METADATA_BYTES_SQL
        ],
        allocation_intent_columns,
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
        reclaim::adopt(connection, root)?;
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
    ownership: WorkOwnership,
    ready_since_ms: u64,
    prepared: Option<OwnerMemberBinding>,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WorkOwnership {
    Open,
    Manual,
    Managed,
}
quanta_index_contract::native_serde_enum!(WorkOwnership {
    units { Open = 0, Manual = 1, Managed = 2 }
    newtypes {}
    records {}
});
impl WorkOwnership {
    const fn column(self) -> i64 {
        match self {
            Self::Open => 0,
            Self::Manual => 1,
            Self::Managed => 2,
        }
    }
}
type WorkSqlRow = (
    Vec<u8>,
    Vec<u8>,
    i64,
    i64,
    i64,
    i64,
    i64,
    i64,
    Vec<u8>,
    Vec<u8>,
);
type OperationSqlRow = (Vec<u8>, i64, Vec<u8>, Vec<u8>);
quanta_index_contract::native_serde_struct!(WorkRow {
    intent: WorkIntent,
    created_operation: OperationKey,
    created_view: ViewRef,
    status: WorkStatus,
    ownership: WorkOwnership,
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
        WorkStatus::Blocked(_) => Ok((4, 0)),
        WorkStatus::Unknown => Err(IndexError::InvalidInput(
            "unknown work status cannot be persisted".into(),
        )),
    }
}

fn renew_claim(
    catalog: &SqliteCatalog,
    claim: &WorkClaim,
    lease: Duration,
    grant: Option<&quanta_index_core::index::OwnedWorkGrant>,
    budget: Option<&WorkBudget>,
) -> Result<WorkClaim, IndexError> {
    let claim = *claim;
    let lease_ms = validated_lease_ms(lease)?;
    let mut guard = match budget {
        Some(budget) => catalog.lock_native_budget(budget)?,
        None => catalog
            .lock()
            .map_err(|error| IndexError::Storage(error.to_string()))?,
    };
    let tx = begin(&mut guard)?;
    let now = catalog_now_ms(&tx, catalog.clock.now_unix_ms())?;
    let (mut row, validated_owned) = live_claim_with_assignment(&tx, &claim, now)?;
    let owned = if let Some(grant) = grant {
        let assigned = match validated_owned {
            Some(assignment) => assignment,
            // Preserve the original owned-row error priority for a malformed
            // grant supplied against a manual claim.
            None => owned_work::read_owned(&tx, &claim.intent)?.ok_or(IndexError::WorkFenceLost)?,
        };
        if row.ownership != WorkOwnership::Managed
            || !assigned.active
            || assigned.request != grant.assignment.request
            || assigned.policy != grant.assignment.policy
            || assigned.deadline_ms != grant.assignment.deadline_ms
            || assigned.dispatches != grant.ordinal
            || assigned.dispatches != grant.assignment.dispatches
        {
            return Err(IndexError::WorkFenceLost);
        }
        Some(assigned)
    } else if row.ownership == WorkOwnership::Managed {
        return Err(IndexError::InvalidInput(
            "managed work requires its current owned grant for renewal".into(),
        ));
    } else {
        None
    };
    let WorkStatus::Claimed(current) = row.status else {
        return Err(IndexError::WorkFenceLost);
    };
    let renewed = WorkClaim {
        intent: claim.intent,
        fence: claim.fence,
        lease_until_ms: current
            .lease_until_ms
            .max(
                now.checked_add(lease_ms)
                    .ok_or(IndexError::ResourceLimited)?,
            )
            .min(
                owned
                    .as_ref()
                    .map_or(u64::MAX, |assignment| assignment.deadline_ms),
            ),
    };
    row.status = WorkStatus::Claimed(renewed);
    write_work(&tx, &row, false)?;
    if let Some(mut assignment) = owned {
        assignment.due_ms = renewed.lease_until_ms;
        owned_work::write_owned(&tx, &assignment, false)?;
    }
    if let Some(budget) = budget {
        budget.checkpoint()?;
    }
    tx.commit().map_err(|error| sql(&error))?;
    drop(guard);
    Ok(renewed)
}

fn bounded_work_blob(
    row: &rusqlite::Row<'_>,
    column: usize,
    max_bytes: u64,
    exact: bool,
) -> rusqlite::Result<Vec<u8>> {
    let value = row.get_ref(column)?;
    let rusqlite::types::ValueRef::Blob(bytes) = value else {
        return Err(rusqlite::Error::InvalidColumnType(
            column,
            "native work blob".into(),
            value.data_type(),
        ));
    };
    let valid = u64::try_from(bytes.len()).is_ok_and(|length| {
        length != 0
            && if exact {
                length == max_bytes
            } else {
                length <= max_bytes
            }
    });
    if !valid {
        let reason = if bytes.is_empty() {
            "empty native work blob"
        } else if exact {
            "native work blob length is not 32 bytes"
        } else {
            "oversized owned work row"
        };
        return Err(rusqlite::Error::FromSqlConversionFailure(
            column,
            rusqlite::types::Type::Blob,
            Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, reason)),
        ));
    }
    Ok(bytes.to_vec())
}

fn read_work(connection: &Connection, intent: &WorkIntent) -> Result<Option<WorkRow>, IndexError> {
    let key = work_key(intent)?;
    read_work_by_key(connection, &key, Some(intent))
}

// Owned ready discovery starts from a bounded key scan. Healthy rows share
// intent-addressed validation; a redirected payload keeps the historical
// decoded-identity lookup and its original refusal priority.
fn read_work_by_ready_key(
    connection: &Connection,
    key: &[u8],
) -> Result<Option<WorkRow>, IndexError> {
    read_work_by_key(connection, key, None)
}

fn read_work_by_key(
    connection: &Connection,
    key: &[u8],
    expected_intent: Option<&WorkIntent>,
) -> Result<Option<WorkRow>, IndexError> {
    let (selected, decoded) = {
        let mut statement = connection
            .prepare(
                "SELECT operation,workspace,epoch,sequence,status,owner_mode,ready_since_ms,lease_until_ms,payload,row_digest
                 FROM native_work_v1 WHERE work=?1",
            )
            .map_err(|error| sql(&error))?;
        let mut rows = statement.query(params![key]).map_err(|error| sql(&error))?;
        let Some(row) = rows.next().map_err(|error| sql(&error))? else {
            return Ok(None);
        };
        // The old ready path checked payload/digest and derived the intent
        // before its second, full-row lookup. Preserve that error priority.
        let prechecked = if expected_intent.is_none() {
            let payload = bounded_work_blob(row, 8, owned_work::MAX_OWNED_WORK_ROW_BYTES, false)
                .map_err(|error| sql(&error))?;
            let digest = bounded_work_blob(row, 9, 32, true).map_err(|error| sql(&error))?;
            check_digest(b"qidx.native.work.v2", &payload, &digest)?;
            let decoded: WorkRow = decode(&payload)?;
            Some((decoded, payload, digest))
        } else {
            None
        };
        if let Some((decoded, _, _)) = &prechecked
            && work_key(&decoded.intent)? != key
        {
            // The old ready path looked up the decoded identity. Keep its
            // wrong-key/missing-row versus existing-other-row distinction.
            let intent = decoded.intent;
            drop(rows);
            drop(statement);
            return read_work(connection, &intent);
        }
        let operation = bounded_work_blob(row, 0, owned_work::MAX_OWNED_WORK_ROW_BYTES, false)
            .map_err(|error| sql(&error))?;
        let workspace = bounded_work_blob(row, 1, owned_work::MAX_OWNED_WORK_ROW_BYTES, false)
            .map_err(|error| sql(&error))?;
        let epoch = row.get(2).map_err(|error| sql(&error))?;
        let sequence = row.get(3).map_err(|error| sql(&error))?;
        let status = row.get(4).map_err(|error| sql(&error))?;
        let owner_mode = row.get(5).map_err(|error| sql(&error))?;
        let ready_since = row.get(6).map_err(|error| sql(&error))?;
        let lease = row.get(7).map_err(|error| sql(&error))?;
        let (decoded, payload, row_digest) = match prechecked {
            Some((decoded, payload, row_digest)) => (Some(decoded), payload, row_digest),
            None => (
                None,
                bounded_work_blob(row, 8, owned_work::MAX_OWNED_WORK_ROW_BYTES, false)
                    .map_err(|error| sql(&error))?,
                bounded_work_blob(row, 9, 32, true).map_err(|error| sql(&error))?,
            ),
        };
        let sql_row: WorkSqlRow = (
            operation,
            workspace,
            epoch,
            sequence,
            status,
            owner_mode,
            ready_since,
            lease,
            payload,
            row_digest,
        );
        (sql_row, decoded)
    };
    validate_work_sql_row(connection, key, expected_intent, selected, decoded).map(Some)
}

fn validate_work_sql_row(
    connection: &Connection,
    key: &[u8],
    expected_intent: Option<&WorkIntent>,
    row: WorkSqlRow,
    prechecked: Option<WorkRow>,
) -> Result<WorkRow, IndexError> {
    let (
        operation,
        workspace,
        epoch,
        sequence,
        status,
        owner_mode,
        ready_since,
        lease,
        payload,
        row_digest,
    ) = row;
    let decoded = if let Some(decoded) = prechecked {
        decoded
    } else {
        check_digest(b"qidx.native.work.v2", &payload, &row_digest)?;
        decode(&payload)?
    };
    let intent = expected_intent.unwrap_or(&decoded.intent);
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
        || decoded.ownership.column() != owner_mode
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
        | WorkStatus::Superseded
        | WorkStatus::Blocked(_) => {}
    }
    for reference in work_roots(&decoded.intent) {
        verify_work_root(connection, &decoded.intent, reference)?;
    }
    if let Some(prepared) = &decoded.prepared {
        verify_work_root(connection, &decoded.intent, prepared.artifact)?;
    }
    Ok(decoded)
}

fn write_work(connection: &Connection, row: &WorkRow, insert: bool) -> Result<(), IndexError> {
    let key = work_key(&row.intent)?;
    let payload = encode(row)?;
    if payload.len()
        > usize::try_from(owned_work::MAX_OWNED_WORK_ROW_BYTES)
            .map_err(|_error| IndexError::ResourceLimited)?
    {
        return Err(IndexError::BackendViolation(
            "owned work row exceeds format bound".into(),
        ));
    }
    let (status, lease) = work_status_columns(&row.status)?;
    let checksum = digest(b"qidx.native.work.v2", &payload).to_vec();
    if insert {
        let inserted = connection.execute(
            "INSERT INTO native_work_v1(work,operation,workspace,epoch,sequence,status,owner_mode,ready_since_ms,lease_until_ms,payload,row_digest)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
            params![key, operation_key(row.created_operation)?, workspace_key(row.intent.workspace)?,
                int(row.intent.epoch)?, int(row.created_view.sequence)?, status, row.ownership.column(), int(row.ready_since_ms)?, lease, payload, checksum],
        ).map_err(|error| sql(&error))?;
        exactly_one(inserted, "insert work intent")?;
    } else {
        let changed = connection.execute(
            "UPDATE native_work_v1 SET status=?2,owner_mode=?3,ready_since_ms=?4,lease_until_ms=?5,payload=?6,row_digest=?7 WHERE work=?1",
            params![key, status, row.ownership.column(), int(row.ready_since_ms)?, lease, payload, checksum],
        ).map_err(|error| sql(&error))?;
        exactly_one(changed, "update work intent")?;
    }
    Ok(())
}

// stage_work changes only the prepared binding. Keep the validated status and
// lease shadows out of the SET list so their ordered indexes are not rewritten.
fn write_prepared_work(connection: &Connection, row: &WorkRow) -> Result<(), IndexError> {
    let key = work_key(&row.intent)?;
    let payload = encode(row)?;
    if payload.len()
        > usize::try_from(owned_work::MAX_OWNED_WORK_ROW_BYTES)
            .map_err(|_error| IndexError::ResourceLimited)?
    {
        return Err(IndexError::BackendViolation(
            "owned work row exceeds format bound".into(),
        ));
    }
    let _status = work_status_columns(&row.status)?;
    let checksum = digest(b"qidx.native.work.v2", &payload).to_vec();
    let changed = connection
        .execute(
            "UPDATE native_work_v1 SET payload=?2,row_digest=?3 WHERE work=?1",
            params![key, payload, checksum],
        )
        .map_err(|error| sql(&error))?;
    exactly_one(changed, "update work intent")?;
    Ok(())
}

/// Authenticate the original durable floor and validate one clock sample.
/// The same checked row governs advisory reads and mutating lease decisions.
fn validated_catalog_clock_sample(
    connection: &Connection,
    current: u64,
) -> Result<(u64, u64), IndexError> {
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
    let floor = uint(last)?;
    if current < floor {
        return Err(IndexError::Storage("catalog clock regressed".into()));
    }
    let _checked = int(current)?;
    Ok((current, floor))
}

/// Advisory reads validate the durable clock floor without issuing a write.
/// Claim/lease mutations recheck and persist time in their own transaction.
fn observed_catalog_now_ms(connection: &Connection, current: u64) -> Result<u64, IndexError> {
    validated_catalog_clock_sample(connection, current).map(|(current, _floor)| current)
}

fn catalog_now_ms(connection: &Connection, current: u64) -> Result<u64, IndexError> {
    let (current, floor) = validated_catalog_clock_sample(connection, current)?;
    persist_validated_catalog_clock_sample(connection, current, floor)
}

// Call only with the authenticated sample from validated_catalog_clock_sample.
// This lets an observational terminal settlement avoid a clock write while
// live claim and lease mutations persist the same sampled time.
fn persist_validated_catalog_clock_sample(
    connection: &Connection,
    current: u64,
    floor: u64,
) -> Result<u64, IndexError> {
    if current == floor {
        // The authenticated sample is already durable; no row needs changing.
        return Ok(current);
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
        let key = bounded_work_blob(row, 0, 32, true).map_err(|error| sql(&error))?;
        let payload_bytes: i64 = row.get(1).map_err(|error| sql(&error))?;
        let due: i64 = row.get(2).map_err(|error| sql(&error))?;
        let payload_bytes = uint(payload_bytes)?;
        if payload_bytes > owned_work::MAX_OWNED_WORK_ROW_BYTES {
            return Err(IndexError::Storage("oversized owned work row".into()));
        }
        budget.charge(WorkUsage {
            reads: 1,
            bytes: payload_bytes
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
    reclaim::adopt(connection, reference)?;
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
    live_claim_with_assignment(connection, claim, now).map(|(row, _assignment)| row)
}

fn live_claim_with_assignment(
    connection: &Connection,
    claim: &WorkClaim,
    now: u64,
) -> Result<(WorkRow, Option<OwnedWorkAssignment>), IndexError> {
    let row = read_work(connection, &claim.intent)?.ok_or(IndexError::WorkFenceLost)?;
    validate_live_claim_row(&row, claim, now)?;
    let assignment = if row.ownership == WorkOwnership::Managed {
        let assignment =
            owned_work::read_owned(connection, &claim.intent)?.ok_or(IndexError::WorkFenceLost)?;
        validate_owned_live_claim(&assignment, claim, now)?;
        Some(assignment)
    } else {
        None
    };
    Ok((row, assignment))
}

fn validate_live_claim_row(row: &WorkRow, claim: &WorkClaim, now: u64) -> Result<(), IndexError> {
    let WorkStatus::Claimed(persisted) = row.status else {
        return Err(IndexError::WorkFenceLost);
    };
    if persisted.intent != claim.intent
        || persisted.fence != claim.fence
        || now >= persisted.lease_until_ms
    {
        return Err(IndexError::WorkFenceLost);
    }
    Ok(())
}

fn validate_owned_live_claim(
    assignment: &OwnedWorkAssignment,
    claim: &WorkClaim,
    now: u64,
) -> Result<(), IndexError> {
    if !assignment.active
        || assignment.dispatches == 0
        || now >= assignment.deadline_ms
        || !matches!(assignment.request.kind, UpdateKind::Materialize { claim: owned }
            if owned.fence == claim.fence && owned.intent == claim.intent)
    {
        return Err(IndexError::WorkFenceLost);
    }
    Ok(())
}

fn validate_materialization_claim(
    connection: &Connection,
    request: &UpdateRequest,
    now: u64,
) -> Result<(), IndexError> {
    let UpdateKind::Materialize { claim } = &request.kind else {
        return Err(IndexError::InvalidInput(
            "materialization validation requires an owner claim".into(),
        ));
    };
    let work = read_work(connection, &claim.intent)?;
    if work.is_some_and(|work| work.ownership == WorkOwnership::Managed) {
        let _live = live_claim(connection, claim, now)?;
        let assignment =
            owned_work::read_owned(connection, &claim.intent)?.ok_or(IndexError::WorkFenceLost)?;
        if assignment.request.operation != request.operation {
            return Err(IndexError::WorkFenceLost);
        }
    } else if request.operation.producer == OWNED_WORK_PRODUCER {
        return Err(IndexError::WorkFenceLost);
    }
    Ok(())
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
    if matches!(
        row.status,
        OperationStatus::Published(_) | OperationStatus::Refused(_)
    ) {
        // The ready set is part of the same terminal-receipt transaction. It
        // is removed only after bounded custody release, never by a scan/TTL.
        let key = row.immutable.key;
        let operation = operation_key(key)?;
        let workspace = workspace_key(key.workspace)?;
        let epoch = int(key.epoch)?;
        // Reuse the existing durable non-reusing counter authority. Producer
        // operation keys cannot order terminal arrivals across streams.
        let ready_sequence = int(next_fence(connection)?)?;
        let checksum = fields_digest(
            b"qidx.native.terminal-custody.v1",
            &(ready_sequence, &operation, &workspace, epoch),
        )?;
        let _queued = connection
            .execute(
                "INSERT INTO native_terminal_custody_v1(operation,ready_sequence,workspace,epoch,row_digest)
             VALUES (?1,?2,?3,?4,?5) ON CONFLICT(operation) DO NOTHING",
                params![operation, ready_sequence, workspace, epoch, checksum],
            )
            .map_err(|error| sql(&error))?;
    }
    Ok(())
}

fn next_fence(connection: &Connection) -> Result<u64, IndexError> {
    fence_counter::next(connection)
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

// The standalone and owned-admission paths share one seal implementation.
// The caller holds the original Catalog transaction; no seal is committed here.
fn seal_ingress_request(
    connection: &Connection,
    ingress: IngressRef,
    request: ObjectRef,
) -> Result<IngressRow, IndexError> {
    let mut row = check_ingress(connection, ingress)?;
    if let Some(prior) = row.sealed {
        if prior != request {
            return Err(IndexError::DigestConflict);
        }
    } else {
        if !confirmed_ingress_object(connection, ingress.operation, request)? {
            return Err(IndexError::InvalidInput(
                "request root is not confirmed by ingress".into(),
            ));
        }
        row.sealed = Some(request);
        write_ingress(connection, row, false)?;
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
    let row: Option<(Vec<u8>, Vec<u8>)> = cached_query_row(
        connection,
        "SELECT CASE WHEN typeof(payload)='blob' AND length(payload) BETWEEN 1 AND ?3 THEN payload END,
                CASE WHEN typeof(row_digest)='blob' AND length(row_digest)=32 THEN row_digest END
         FROM native_ingress_intent_v1 WHERE operation=?1 AND object=?2",
        params![&operation, &object_key, object_install::MAX_METADATA_BYTES_SQL],
        allocation_intent_columns,
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
    if !confirmed_ingress_membership(connection, key, reference)? {
        return Ok(false);
    }
    require_object(connection, reference)?;
    Ok(true)
}

/// Authenticate only the operation's custody row. Registration is checked by
/// the generic caller or supplied by the exact current-transaction batch proof.
fn confirmed_ingress_membership(
    connection: &Connection,
    key: OperationKey,
    reference: ObjectRef,
) -> Result<bool, IndexError> {
    let operation = operation_key(key)?;
    let object = object_key(reference);
    let checksum: Option<Vec<u8>> = cached_query_row(
        connection,
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
        if reclaim::is_bound(connection)? {
            budget.charge(WorkUsage {
                reads: rows.checked_mul(10).ok_or(IndexError::ResourceLimited)?,
                writes: rows,
                bytes: rows.checked_mul(640).ok_or(IndexError::ResourceLimited)?,
                mutations: rows,
            })?;
            for object in &batch {
                reclaim::release_key(connection, object)?;
            }
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
        if operation.producer == OWNED_WORK_PRODUCER {
            let now = catalog_now_ms(&tx, self.clock.now_unix_ms())?;
            owned_work::validate_reserved_key(&tx, operation, now)?;
        }
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

    fn adopt_registered_inputs(
        &self,
        ingress: IngressRef,
        objects: &[RegisteredObject],
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        managed_install::adopt_registered_inputs(self, ingress, objects, budget)
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
        reclaim::require_unbound(&tx)?;
        require_open_ingress(&tx, ingress)?;
        let mut earlier = std::collections::BTreeSet::new();
        for object in objects {
            let canonical = canonical_object(object)?;
            for child in &canonical.children {
                if !earlier.contains(child) {
                    require_object(&tx, *child)?;
                }
            }
            object_install::reserve(&tx, &canonical)?;
            if !earlier.insert(object.reference) {
                return Err(IndexError::InvalidInput(
                    "duplicate ingress reservation".into(),
                ));
            }
            let payload = encode(&canonical)?;
            if payload.len() > 1_048_576 {
                return Err(IndexError::ResourceLimited);
            }
            if !try_insert_ingress_intent(&tx, ingress.operation, &canonical, &payload)? {
                require_reserved_ingress_intent(&tx, ingress.operation, &canonical, &payload)?;
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
        reclaim::require_unbound(&tx)?;
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
        register_confirmed(&tx, objects)?;
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
        let _sealed = seal_ingress_request(&tx, ingress, request)?;
        tx.commit().map_err(|error| sql(&error))?;
        drop(guard);
        Ok(())
    }

    fn discard_inputs(
        &self,
        ingress: IngressRef,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        let mut guard = self.lock_native_budget(budget)?;
        let tx = begin(&mut guard)?;
        let _checked = check_ingress(&tx, ingress)?;
        delete_ingress(&tx, ingress.operation, budget)?;
        budget.checkpoint()?;
        tx.commit().map_err(|error| sql(&error))?;
        drop(guard);
        Ok(())
    }
}

impl SqliteCatalog {
    fn admit_common(
        &self,
        immutable: ImmutableOperation,
        ingress: IngressRef,
        events: &[SourceEvent],
        owned: Option<&UpdateRequest>,
        budget: &mut WorkBudget,
    ) -> Result<AdmissionOutcome, IndexError> {
        let mut guard = self.lock_native_budget(budget)?;
        let tx = begin(&mut guard)?;
        match owned {
            Some(request) => {
                if immutable.key.producer != OWNED_WORK_PRODUCER
                    || request.operation != immutable.key
                    || !events.is_empty()
                    || immutable.source_mode != SourceOperationMode::Unchanged
                {
                    return Err(IndexError::InvalidInput(
                        "owned admission identity mismatch".into(),
                    ));
                }
                let now = catalog_now_ms(&tx, self.clock.now_unix_ms())?;
                owned_work::validate_request(&tx, request, now)?;
            }
            None if immutable.key.producer == OWNED_WORK_PRODUCER => {
                return Err(IndexError::InvalidInput(
                    "reserved operation requires owned admission".into(),
                ));
            }
            None => {}
        }
        if let Some(existing) = read_operation(&tx, immutable.key)? {
            if existing.immutable != immutable || existing.events != events {
                return Err(IndexError::DigestConflict);
            }
            return Ok(AdmissionOutcome::Existing(existing.status));
        }
        if ingress.operation != immutable.key {
            return Err(IndexError::InvalidInput(
                "ingress belongs to another operation".into(),
            ));
        }
        let staged = if owned.is_some() {
            seal_ingress_request(&tx, ingress, immutable.request)?
        } else {
            check_ingress(&tx, ingress)?
        };
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
        source_authority::admit(&tx, immutable.key, immutable.source_mode, events, budget)?;
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
}

impl OperationJournalPort for SqliteCatalog {
    fn admit(
        &self,
        immutable: ImmutableOperation,
        ingress: IngressRef,
        events: &[SourceEvent],
        budget: &mut WorkBudget,
    ) -> Result<AdmissionOutcome, IndexError> {
        self.admit_common(immutable, ingress, events, None, budget)
    }

    fn admit_owned(
        &self,
        immutable: ImmutableOperation,
        ingress: IngressRef,
        request: &UpdateRequest,
        identity: &dyn ImmutableObjectPort,
        budget: &mut WorkBudget,
    ) -> Result<AdmissionOutcome, IndexError> {
        let UpdateKind::Materialize { claim } = &request.kind else {
            return Err(IndexError::InvalidInput(
                "owned admission requires materialize".into(),
            ));
        };
        let mut children = match claim.intent.scope {
            WorkScope::Owner { expected, .. } => expected
                .source
                .into_iter()
                .chain(expected.projection)
                .collect::<Vec<_>>(),
            WorkScope::ProjectionDomain { .. } => Vec::new(),
        };
        children.sort_unstable();
        children.dedup();
        let identified = identity.identify(&encode(request)?, &children, budget)?;
        if identified.reference != immutable.request {
            return Err(IndexError::DigestConflict);
        }
        self.admit_common(immutable, ingress, &[], Some(request), budget)
    }

    fn inspect(
        &self,
        key: OperationKey,
        access: OperationInspectAccess<'_>,
    ) -> Result<OperationStatus, IndexError> {
        match access {
            OperationInspectAccess::Request(budget) => {
                let guard = self.lock_native_budget(budget)?;
                let result = read_operation(&guard, key);
                drop(guard);
                let status = result?.map_or(OperationStatus::Unknown, |row| row.status);
                budget.checkpoint()?;
                Ok(status)
            }
            OperationInspectAccess::PostFailureReconcile => {
                let guard = self
                    .lock()
                    .map_err(|error| IndexError::Storage(error.to_string()))?;
                Ok(read_operation(&guard, key)?.map_or(OperationStatus::Unknown, |row| row.status))
            }
        }
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
        reclaim::require_unbound(&tx)?;
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
        reclaim::require_unbound(&tx)?;
        let _checked_attempt = check_attempt(&tx, attempt)?;
        for object in objects {
            require_intent(&tx, attempt, object)?;
        }
        register_confirmed(&tx, objects)?;
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
    fn ready_owned_work(
        &self,
        workspace: WorkspaceKey,
        epoch: u64,
        limit: u32,
        budget: &mut WorkBudget,
    ) -> Result<quanta_index_core::index::OwnedReadyOutcome, IndexError> {
        self.ready_owned_work_impl(workspace, epoch, limit, budget)
    }

    fn acquire_owned_work(
        &self,
        intent: &WorkIntent,
        policy: quanta_index_core::index::OwnedWorkPolicy,
        budget: &WorkBudget,
    ) -> Result<Option<quanta_index_core::index::OwnedWorkAssignment>, IndexError> {
        self.acquire_owned_work_impl(intent, policy, budget)
    }

    fn begin_owned_work(
        &self,
        assignment: &quanta_index_core::index::OwnedWorkAssignment,
    ) -> Result<quanta_index_core::index::OwnedWorkBegin, IndexError> {
        self.begin_owned_work_impl(assignment)
    }

    fn finish_owned_work(
        &self,
        grant: &quanta_index_core::index::OwnedWorkGrant,
        report: quanta_index_core::index::OwnedWorkReport,
    ) -> Result<(), IndexError> {
        self.finish_owned_work_impl(grant, report)
    }

    fn validate_owned_work(&self, request: &UpdateRequest) -> Result<OperationStatus, IndexError> {
        self.validate_owned_work_impl(request)
    }

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
        let mut guard = self.lock_native_budget(budget)?;
        let tx = guard
            .transaction_with_behavior(TransactionBehavior::Deferred)
            .map_err(|error| sql(&error))?;
        let current = current_view(&tx, workspace)?.ok_or(IndexError::SnapshotUnavailable)?;
        if current.identity.epoch != epoch {
            return Err(IndexError::Conflict);
        }
        let now = observed_catalog_now_ms(&tx, self.clock.now_unix_ms())?;
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
                    |row| {
                        Ok((
                            bounded_work_blob(row, 0, owned_work::MAX_OWNED_WORK_ROW_BYTES, false)?,
                            bounded_work_blob(row, 1, 32, true)?,
                        ))
                    },
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
                | WorkStatus::Superseded
                | WorkStatus::Blocked(_) => {
                    return Err(IndexError::Storage("ready work status mismatch".into()));
                }
            }
            result.push(verified.intent);
        }
        tx.commit().map_err(|error| sql(&error))?;
        drop(guard);
        budget.checkpoint()?;
        Ok(result)
    }

    fn claim_work(
        &self,
        intent: &WorkIntent,
        lease: Duration,
        budget: &WorkBudget,
    ) -> Result<WorkClaim, IndexError> {
        let intent = *intent;
        let lease_ms = validated_lease_ms(lease)?;
        let mut guard = self.lock_native_budget(budget)?;
        let tx = begin(&mut guard)?;
        let current =
            current_view(&tx, intent.workspace)?.ok_or(IndexError::SnapshotUnavailable)?;
        if current.identity.epoch != intent.epoch {
            return Err(IndexError::Conflict);
        }
        let now = catalog_now_ms(&tx, self.clock.now_unix_ms())?;
        let mut row = read_work(&tx, &intent)?.ok_or(IndexError::Conflict)?;
        if row.ownership == WorkOwnership::Managed {
            return Err(IndexError::InvalidInput(
                "managed work cannot be claimed manually".into(),
            ));
        }
        match row.status {
            WorkStatus::Pending => {}
            WorkStatus::Claimed(old) if old.lease_until_ms <= now => {}
            WorkStatus::Unknown
            | WorkStatus::Claimed(_)
            | WorkStatus::Completed(_)
            | WorkStatus::Superseded
            | WorkStatus::Blocked(_) => return Err(IndexError::Conflict),
        }
        let claim = WorkClaim {
            intent,
            fence: next_fence(&tx)?,
            lease_until_ms: now
                .checked_add(lease_ms)
                .ok_or(IndexError::ResourceLimited)?,
        };
        row.status = WorkStatus::Claimed(claim);
        row.ownership = WorkOwnership::Manual;
        write_work(&tx, &row, false)?;
        budget.checkpoint()?;
        tx.commit().map_err(|error| sql(&error))?;
        drop(guard);
        Ok(claim)
    }

    fn renew_work(
        &self,
        claim: &WorkClaim,
        lease: Duration,
        budget: &WorkBudget,
    ) -> Result<WorkClaim, IndexError> {
        renew_claim(self, claim, lease, None, Some(budget))
    }

    fn renew_owned_work(
        &self,
        grant: &quanta_index_core::index::OwnedWorkGrant,
    ) -> Result<WorkClaim, IndexError> {
        let UpdateKind::Materialize { claim } = &grant.assignment.request.kind else {
            return Err(IndexError::InvalidInput(
                "owned renewal grant must materialize".into(),
            ));
        };
        renew_claim(
            self,
            claim,
            Duration::from_millis(grant.assignment.policy.lease_ms),
            Some(grant),
            None,
        )
    }

    fn validate_materialization(&self, request: &UpdateRequest) -> Result<(), IndexError> {
        let UpdateKind::Materialize { .. } = &request.kind else {
            return Err(IndexError::InvalidInput(
                "materialization validation requires an owner claim".into(),
            ));
        };
        let key = request.operation;
        let mut guard = self
            .lock()
            .map_err(|error| IndexError::Storage(error.to_string()))?;
        let tx = begin(&mut guard)?;
        let now = catalog_now_ms(&tx, self.clock.now_unix_ms())?;
        if key.producer == OWNED_WORK_PRODUCER {
            // Keep exact-body validation before the live-claim checks, in the
            // same durable clock transaction instead of a preceding COMMIT.
            owned_work::validate_request(&tx, request, now)?;
        }
        validate_materialization_claim(&tx, request, now)?;
        tx.commit().map_err(|error| sql(&error))?;
        drop(guard);
        Ok(())
    }

    fn work_status(
        &self,
        intent: &WorkIntent,
        budget: &WorkBudget,
    ) -> Result<WorkStatus, IndexError> {
        let intent = *intent;
        let guard = self.lock_native_budget(budget)?;
        let status = read_work(&guard, &intent)?.map_or(WorkStatus::Unknown, |row| row.status);
        drop(guard);
        budget.checkpoint()?;
        Ok(status)
    }

    fn acquire_materialization_ticket(
        &self,
        session: &dyn quanta_index_core::index::ReadSessionLease,
        request: &UpdateRequest,
        lease: Duration,
    ) -> Result<quanta_index_core::index::TicketRecord, IndexError> {
        self.acquire_materialization_ticket_impl(session, request, lease)
    }

    fn prepared_work(&self, claim: &WorkClaim) -> Result<Option<OwnerMemberBinding>, IndexError> {
        let claim = *claim;
        let mut guard = self
            .lock()
            .map_err(|error| IndexError::Storage(error.to_string()))?;
        let tx = begin(&mut guard)?;
        let now = observed_catalog_now_ms(&tx, self.clock.now_unix_ms())?;
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
        let now = catalog_now_ms(&tx, self.clock.now_unix_ms())?;
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
        write_prepared_work(&tx, &row)?;
        tx.commit().map_err(|error| sql(&error))?;
        drop(guard);
        Ok(())
    }
}

fn install_frontiers(
    connection: &Connection,
    workspace: WorkspaceKey,
    events: &[SourceEvent],
    mode: SourceOperationMode,
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
                |row| {
                    let value = row.get_ref(2)?;
                    let rusqlite::types::ValueRef::Blob(digest) = value else {
                        return Err(rusqlite::Error::InvalidColumnType(
                            2,
                            "row_digest".into(),
                            value.data_type(),
                        ));
                    };
                    if digest.len() != 32 {
                        return Err(rusqlite::Error::InvalidColumnType(
                            2,
                            "row_digest".into(),
                            rusqlite::types::Type::Blob,
                        ));
                    }
                    Ok((row.get(0)?, row.get(1)?, digest.to_vec()))
                },
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
                if !matches!(mode, SourceOperationMode::Resync { .. })
                    && uint(epoch)? == event.stream_epoch
                    && uint(sequence)? == event.predecessor => {}
            Some((epoch, _, _))
                if !matches!(mode, SourceOperationMode::Unchanged)
                    && uint(epoch)? < event.stream_epoch
                    && event.predecessor == 0
                    && event.sequence == 1 => {}
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

impl CatalogGenesisPort for SqliteCatalog {
    fn begin_bootstrap(
        &self,
        workspace: WorkspaceKey,
        epoch: u64,
        identity: &dyn ImmutableObjectPort,
        budget: &mut WorkBudget,
    ) -> Result<BootstrapRef, IndexError> {
        let mut guard = self.lock_native_budget(budget)?;
        bootstrap::begin_bootstrap(&mut guard, workspace, epoch, identity, budget)
    }

    fn reserve_bootstrap(
        &self,
        stage: BootstrapRef,
        object: &RegisteredObject,
    ) -> Result<(), IndexError> {
        let mut guard = self
            .lock()
            .map_err(|error| IndexError::Storage(error.to_string()))?;
        bootstrap::reserve_bootstrap(&mut guard, stage, object)
    }

    fn confirm_bootstrap(
        &self,
        stage: BootstrapRef,
        object: &RegisteredObject,
    ) -> Result<(), IndexError> {
        let mut guard = self
            .lock()
            .map_err(|error| IndexError::Storage(error.to_string()))?;
        bootstrap::confirm_bootstrap(&mut guard, stage, object)
    }

    fn publish_bootstrap(
        &self,
        stage: BootstrapRef,
        genesis: CatalogView,
        budget: &mut WorkBudget,
    ) -> Result<ViewRef, IndexError> {
        let mut guard = self.lock_native_budget(budget)?;
        bootstrap::publish_bootstrap(&mut guard, stage, &genesis, budget)
    }

    fn abort_bootstrap(
        &self,
        stage: BootstrapRef,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        let mut guard = self.lock_native_budget(budget)?;
        bootstrap::abort_bootstrap(&mut guard, stage, budget)
    }
}

impl CatalogPublicationPort for SqliteCatalog {
    fn commit_transition(
        &self,
        publication: PreparedPublication,
        actions: &mut dyn WorkActionCursor,
        budget: &mut WorkBudget,
    ) -> Result<OperationStatus, IndexError> {
        budget.checkpoint()?;
        let mut guard = self.lock_native_budget(budget)?;
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
        if row.immutable.source_mode != publication.source_mode {
            return Err(IndexError::InvalidInput(
                "publication source mode differs from admitted operation".into(),
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
        source_authority::commit(
            &tx,
            publication.attempt.operation,
            publication.source_mode,
            &publication.source_events,
            budget,
        )?;
        transition_claims(
            &tx,
            publication.attempt.operation,
            Some(admitted_events),
            EventClaimState::Committed,
        )?;
        install_frontiers(
            &tx,
            current.identity.workspace,
            admitted_events,
            publication.source_mode,
        )?;
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
        // The current attempt's frozen producer stream is consumed in this
        // transaction. The committed WorkRows and their root custody, not a
        // transient action manifest, are the durable work authority.
        // An authenticated empty stream cannot collide with complete_work.
        // Avoid changing the temp schema for each owner-only publication;
        // still consume EOF and validate its count/digest below.
        let track_action_keys = work_set.count != 0;
        if track_action_keys {
            tx.execute_batch(
                "CREATE TEMP TABLE native_work_action_seen_v1(work BLOB PRIMARY KEY) WITHOUT ROWID",
            )
            .map_err(|error| sql(&error))?;
        }
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
                        owned_work::park_owned(&tx, &intent)?;
                    }
                }
                WorkAction::Create(_) => {
                    validate_work_scope(&tx, &intent)?;
                    let since = if let Some(since) = ready_since_ms {
                        since
                    } else {
                        let since = catalog_now_ms(&tx, self.clock.now_unix_ms())?;
                        ready_since_ms = Some(since);
                        since
                    };
                    if let Some(mut prior) = read_work(&tx, &intent)? {
                        if matches!(
                            prior.status,
                            WorkStatus::Completed(_)
                                | WorkStatus::Superseded
                                | WorkStatus::Blocked(_)
                        ) {
                            owned_work::delete_owned(&tx, &intent)?;
                            prior.status = WorkStatus::Pending;
                            prior.ownership = WorkOwnership::Open;
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
                                ownership: WorkOwnership::Open,
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
            if track_action_keys {
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
            }
            let now = catalog_now_ms(&tx, self.clock.now_unix_ms())?;
            let (mut completed, assignment) = live_claim_with_assignment(&tx, &claim, now)?;
            if completed.ownership == WorkOwnership::Managed {
                let assignment = assignment.as_ref().ok_or(IndexError::WorkFenceLost)?;
                if assignment.request.operation != publication.attempt.operation {
                    return Err(IndexError::WorkFenceLost);
                }
                owned_work::validate_request_from_live(
                    &tx,
                    &assignment.request,
                    now,
                    &completed,
                    assignment,
                )?;
            }
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
            if let Some(assignment) = assignment.as_ref() {
                owned_work::park_completed_owned(&tx, assignment)?;
            } else {
                owned_work::park_owned(&tx, &claim.intent)?;
            }
        }
        if track_action_keys {
            tx.execute_batch("DROP TABLE temp.native_work_action_seen_v1")
                .map_err(|error| sql(&error))?;
        }
        row.status = OperationStatus::Published(next.identity);
        budget.charge(WorkUsage {
            reads: 1,
            writes: 2,
            mutations: 2,
            bytes: 2048,
        })?;
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

impl ManagedObjectInstallPort for SqliteCatalog {
    fn reserve_managed(
        &self,
        scope: ObjectInstallScope,
        objects: &[RegisteredObject],
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        managed_install::install(self, scope, objects, budget, false)
    }

    fn confirm_managed(
        &self,
        scope: ObjectInstallScope,
        objects: &[RegisteredObject],
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        managed_install::install(self, scope, objects, budget, true)
    }
}

impl CatalogReclaimPort for SqliteCatalog {
    fn reclaim_work_page(
        &self,
        after: Option<ObjectRef>,
        limit: u32,
        budget: &mut WorkBudget,
    ) -> Result<ReclaimWorkPage, IndexError> {
        reclaim::work_page(self, after, limit, budget)
    }

    fn retire_candidate(
        &self,
        candidate: ReclaimCandidate,
        budget: &mut WorkBudget,
    ) -> Result<Option<RetiringRef>, IndexError> {
        reclaim::retire(self, candidate, budget)
    }

    fn enter_reclaiming(
        &self,
        retiring: RetiringRef,
        target: ReclaimTarget,
        budget: &mut WorkBudget,
    ) -> Result<ReclaimPermit, IndexError> {
        reclaim::enter(self, retiring, target, budget)
    }

    fn validate_reclaiming(
        &self,
        permit: ReclaimPermit,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        reclaim::validate(self, &permit, budget)
    }

    fn mark_unlinked(
        &self,
        permit: ReclaimPermit,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        reclaim::mark_unlinked(self, &permit, budget)
    }

    fn drain_unlinked(
        &self,
        permit: ReclaimPermit,
        limit: u32,
        budget: &mut WorkBudget,
    ) -> Result<ReclaimDrainPage, IndexError> {
        reclaim::drain(self, &permit, limit, budget)
    }
}

impl ManagedObjectAuthority for SqliteCatalog {
    fn bind_namespace(&self, binding: ObjectNamespaceBinding) -> Result<(), IndexError> {
        if binding.root_inode == 0
            || binding.objects_inode == 0
            || binding.lease_inode == 0
            || binding.root_device != binding.objects_device
            || binding.root_device != binding.lease_device
        {
            return Err(IndexError::InvalidInput(
                "managed object namespace binding is invalid".into(),
            ));
        }
        let payload = encode(&(
            binding.root_device,
            binding.root_inode,
            binding.objects_device,
            binding.objects_inode,
            binding.lease_device,
            binding.lease_inode,
        ))?;
        if payload.len() > 256 {
            return Err(IndexError::ResourceLimited);
        }
        let mut guard = self
            .lock()
            .map_err(|error| IndexError::Storage(error.to_string()))?;
        let tx = begin(&mut guard)?;
        let lengths: Option<(i64, i64)> = tx
            .query_row(
                "SELECT length(payload),length(row_digest) FROM native_gc_namespace_v1 WHERE singleton=1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|error| sql(&error))?;
        if let Some((payload_len, digest_len)) = lengths {
            if !(1..=256).contains(&payload_len) || digest_len != 32 {
                return Err(IndexError::Storage(
                    "managed object namespace binding shape mismatch".into(),
                ));
            }
            let (old_payload, old_digest): (Vec<u8>, Vec<u8>) = tx
                .query_row(
                    "SELECT payload,row_digest FROM native_gc_namespace_v1 WHERE singleton=1",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .map_err(|error| sql(&error))?;
            if old_payload.len()
                != usize::try_from(payload_len).map_err(|_error| IndexError::ResourceLimited)?
            {
                return Err(IndexError::Storage(
                    "managed namespace binding changed".into(),
                ));
            }
            check_digest(b"qidx.native.gc.namespace.v1", &old_payload, &old_digest)?;
            if old_payload != payload {
                return Err(IndexError::Conflict);
            }
        } else {
            // A managed namespace cannot adopt a legacy object corpus without
            // complete per-object lifecycle rows and queued orphan custody.
            let unmanaged: i64 = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM native_object_v1 UNION ALL SELECT 1 FROM native_object_install_v1 UNION ALL SELECT 1 FROM native_object_edge_v1 UNION ALL SELECT 1 FROM native_object_install_edge_v1 UNION ALL SELECT 1 FROM native_gc_object_v1 UNION ALL SELECT 1 FROM native_gc_queue_v1 LIMIT 1)",
                [], |row| row.get(0),
            ).map_err(|error| sql(&error))?;
            if unmanaged != 0 {
                return Err(IndexError::RebuildRequired);
            }
            let inserted = tx.execute(
                "INSERT INTO native_gc_namespace_v1(singleton,payload,row_digest) VALUES (1,?1,?2)",
                params![&payload, digest(b"qidx.native.gc.namespace.v1", &payload).to_vec()],
            ).map_err(|error| sql(&error))?;
            exactly_one(inserted, "bind managed object namespace")?;
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
        sync::{
            Arc,
            atomic::{AtomicBool, AtomicU64, Ordering},
        },
        time::{Duration, Instant},
    };

    use quanta_index_core::index::ports::{
        CatalogCustodyMaintenancePort, CatalogExactHistoryPort, CatalogReadPort,
        CatalogTerminalWorkRetentionPort, SourceAuthorityPort,
    };
    use quanta_index_core::index::{
        CollectionId, Digest, IndexInstanceId, MapRoot, OwnedReadyOutcome, OwnedWorkAssignment,
        OwnedWorkBegin, OwnedWorkGrant, OwnedWorkPolicy, OwnedWorkReport, OwnedWorkTerminal,
        OwnerInputBinding, OwnerKey, ProducerId, ProjectionId, SlotId, SourceCut, StableOwnerId,
        StorageFormatId, StreamId, TenantId, ViewSelection, WorkActionSet, WorkBudgetDenial,
        WorkLimits, WorktreeId,
    };

    use super::*;
    use crate::connection::CatalogClockPort;

    type TestResult = Result<(), Box<dyn Error>>;

    fn require_owned_grant(
        decision: OwnedWorkBegin,
        message: &'static str,
    ) -> Result<OwnedWorkGrant, Box<dyn Error>> {
        match decision {
            OwnedWorkBegin::Granted(grant) => Ok(grant),
            OwnedWorkBegin::Deferred(reason) => {
                Err(format!("{message}: deferred {reason:?}").into())
            }
            OwnedWorkBegin::Terminal(status) => {
                Err(format!("{message}: terminal {status:?}").into())
            }
        }
    }

    fn require_owned_ready(outcome: OwnedReadyOutcome) -> Result<Vec<WorkIntent>, Box<dyn Error>> {
        match outcome {
            OwnedReadyOutcome::Ready(ready) => Ok(ready),
            OwnedReadyOutcome::RetryDeadline => Err("owned discovery unexpectedly expired".into()),
        }
    }

    // Keep test failures fallible in Result-returning tests and helpers.
    // The borrowed match operands have the same single-evaluation behavior as assert_eq!.
    macro_rules! check {
        ($condition:expr $(,)?) => {{
            if !$condition {
                return Err(format!("assertion failed: {}", stringify!($condition)).into());
            }
        }};
        ($condition:expr, $($message:tt)+) => {{
            if !$condition {
                return Err(format!($($message)+).into());
            }
        }};
    }
    macro_rules! check_eq {
        ($left:expr, $right:expr $(,)?) => {{
            match (&$left, &$right) {
                (left, right) if !(*left == *right) => {
                    return Err(format!("assertion `left == right` failed\n left: {left:?}\n right: {right:?}").into());
                }
                _ => {}
            }
        }};
        ($left:expr, $right:expr, $($message:tt)+) => {{
            match (&$left, &$right) {
                (left, right) if !(*left == *right) => {
                    return Err(format!("assertion `left == right` failed\n left: {left:?}\n right: {right:?}: {}", format_args!($($message)+)).into());
                }
                _ => {}
            }
        }};
    }
    macro_rules! check_ne {
        ($left:expr, $right:expr $(,)?) => {{
            match (&$left, &$right) {
                (left, right) if !(*left != *right) => {
                    return Err(format!("assertion `left != right` failed\n left: {left:?}\n right: {right:?}").into());
                }
                _ => {}
            }
        }};
        ($left:expr, $right:expr, $($message:tt)+) => {{
            match (&$left, &$right) {
                (left, right) if !(*left != *right) => {
                    return Err(format!("assertion `left != right` failed\n left: {left:?}\n right: {right:?}: {}", format_args!($($message)+)).into());
                }
                _ => {}
            }
        }};
    }

    mod prechecked_children;

    thread_local! {
        static OWNED_RENEW_READS: std::cell::Cell<(u64, u64)> = const {
            std::cell::Cell::new((0, 0))
        };
    }

    fn observe_owned_renew_reads(event: rusqlite::trace::TraceEvent<'_>) {
        let rusqlite::trace::TraceEvent::Stmt(_statement, sql) = event else {
            return;
        };
        OWNED_RENEW_READS.with(|counter| {
            let mut reads = counter.get();
            if sql.starts_with("SELECT") && sql.contains("FROM native_owned_work_v1 WHERE work=") {
                reads.0 = reads.0.saturating_add(1);
            } else if sql.starts_with("SELECT")
                && sql.contains("FROM native_owned_request_v1 WHERE operation=")
            {
                reads.1 = reads.1.saturating_add(1);
            }
            counter.set(reads);
        });
    }

    type MaterializationReads = (u64, u64, u64);

    thread_local! {
        static OWNED_MATERIALIZATION_READS: std::cell::Cell<(u64, u64, u64)> = const {
            std::cell::Cell::new((0, 0, 0))
        };
        static OWNED_OPERATION_READS: std::cell::Cell<u64> = const {
            std::cell::Cell::new(0)
        };
    }

    fn observe_owned_materialization_reads(event: rusqlite::trace::TraceEvent<'_>) {
        let rusqlite::trace::TraceEvent::Stmt(_statement, sql) = event else {
            return;
        };
        OWNED_MATERIALIZATION_READS.with(|counter| {
            let mut reads = counter.get();
            if sql.starts_with("SELECT") && sql.contains("FROM native_work_v1 WHERE work=") {
                reads.0 = reads.0.saturating_add(1);
            } else if sql.starts_with("SELECT")
                && sql.contains("FROM native_owned_work_v1 WHERE work=")
            {
                reads.1 = reads.1.saturating_add(1);
            } else if sql.starts_with("SELECT")
                && sql.contains("FROM native_owned_request_v1 WHERE operation=")
            {
                reads.2 = reads.2.saturating_add(1);
            }
            counter.set(reads);
        });
        if sql.starts_with("SELECT") && sql.contains("FROM native_operation_v1 WHERE operation=") {
            OWNED_OPERATION_READS.with(|counter| counter.set(counter.get().saturating_add(1)));
        }
    }

    fn counted_owned_materialization<T>(
        catalog: &SqliteCatalog,
        action: impl FnOnce() -> Result<T, IndexError>,
    ) -> Result<(T, MaterializationReads), Box<dyn Error>> {
        OWNED_MATERIALIZATION_READS.with(|counter| counter.set((0, 0, 0)));
        OWNED_OPERATION_READS.with(|counter| counter.set(0));
        let guard = catalog.lock()?;
        guard.trace_v2(
            rusqlite::trace::TraceEventCodes::SQLITE_TRACE_STMT,
            Some(observe_owned_materialization_reads),
        );
        drop(guard);
        let result = action();
        let guard = catalog.lock()?;
        guard.trace_v2(rusqlite::trace::TraceEventCodes::empty(), None);
        drop(guard);
        let reads = OWNED_MATERIALIZATION_READS.with(std::cell::Cell::get);
        Ok((result?, reads))
    }

    fn fixture_current_view(catalog: &SqliteCatalog) -> Result<CatalogView, Box<dyn Error>> {
        let guard = catalog.lock()?;
        let view = current_view(&guard, workspace())?.ok_or("missing current view")?;
        drop(guard);
        Ok(view)
    }

    fn id(value: u8) -> [u8; 16] {
        [value; 16]
    }
    fn digest_id(value: u8) -> Digest {
        Digest([value; 32])
    }
    fn object(value: u8, children: Vec<ObjectRef>) -> RegisteredObject {
        RegisteredObject::unscoped(ObjectRef(digest_id(value)), 1, children)
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
                digest: digest_id(0),
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
        // Generic journal fixture; source-authority tests opt into Advance.
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
            source_mode: SourceOperationMode::Unchanged,
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
    fn owner_result(
        intent: &WorkIntent,
        artifact: ObjectRef,
    ) -> Result<OwnerMemberBinding, &'static str> {
        let WorkScope::Owner { owner, expected } = intent.scope else {
            return Err("fixture only creates owner work");
        };
        Ok(OwnerMemberBinding {
            instance: intent.instance,
            owner,
            input: expected,
            format: StorageFormatId(id(41)),
            artifact,
            logical_commitment: digest_id(42),
        })
    }
    struct ManualClock(AtomicU64);

    impl ManualClock {
        fn new(now: u64) -> Self {
            Self(AtomicU64::new(now))
        }

        fn set(&self, now: u64) {
            self.0.store(now, Ordering::SeqCst);
        }
    }

    impl CatalogClockPort for ManualClock {
        fn now_unix_ms(&self) -> u64 {
            self.0.load(Ordering::SeqCst)
        }
    }

    struct CancelOnNextClock {
        now: u64,
        armed: AtomicBool,
        stop: Arc<AtomicBool>,
    }

    impl CatalogClockPort for CancelOnNextClock {
        fn now_unix_ms(&self) -> u64 {
            if self.armed.swap(false, Ordering::AcqRel) {
                self.stop.store(true, Ordering::Release);
            }
            self.now
        }
    }

    fn open(root: &Path) -> Result<SqliteCatalog, CoreError> {
        SqliteCatalog::open(root, Duration::from_secs(1))
    }
    fn setup(root: &Path) -> Result<(SqliteCatalog, CatalogView), Box<dyn Error>> {
        setup_with_clock(root, Arc::new(crate::connection::SystemCatalogClock))
    }
    fn setup_with_clock(
        root: &Path,
        clock: Arc<dyn CatalogClockPort>,
    ) -> Result<(SqliteCatalog, CatalogView), Box<dyn Error>> {
        let catalog = SqliteCatalog::open_with_clock(root, Duration::from_secs(1), clock)?;
        let profile = object(30, vec![]);
        let genesis = view(0, profile.reference, None);
        let stage = catalog.begin_bootstrap(
            genesis.identity.workspace,
            genesis.identity.epoch,
            &GenesisIdentity(profile.clone()),
            &mut admission_budget(),
        )?;
        catalog.reserve_bootstrap(stage, &profile)?;
        catalog.confirm_bootstrap(stage, &profile)?;
        let created = catalog.publish_bootstrap(stage, genesis.clone(), &mut admission_budget())?;
        check_eq!(created, genesis.identity);
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
        let request = RegisteredObject::unscoped(operation.request, 1, vec![]);
        let ingress = stage_inputs(catalog, operation, &[request])?;
        match catalog.admit(operation, ingress, events, &mut admission_budget())? {
            AdmissionOutcome::New(attempt) => Ok(attempt),
            AdmissionOutcome::Existing(status) => {
                Err(format!("expected new attempt, got existing {status:?}").into())
            }
        }
    }
    fn admission_budget() -> WorkBudget {
        WorkBudget::with_request(
            WorkLimits {
                reads: 100_000,
                writes: 100_000,
                bytes: 32 * 1024 * 1024,
                mutations: 100_000,
            },
            quanta_index_core::RequestBudgetV1::for_duration(Duration::from_secs(30)),
        )
    }

    fn inspect_with_test_budget(
        catalog: &SqliteCatalog,
        key: OperationKey,
    ) -> Result<OperationStatus, IndexError> {
        let budget = admission_budget();
        catalog.inspect(key, OperationInspectAccess::Request(&budget))
    }

    fn acquire_ticket_with_test_budget(
        catalog: &SqliteCatalog,
        session: &dyn quanta_index_core::index::ReadSessionLease,
        selection: ViewSelection,
        lease: Duration,
    ) -> Result<quanta_index_core::index::TicketRecord, IndexError> {
        let budget = admission_budget();
        catalog.acquire_ticket(session, selection, lease, &budget)
    }

    fn fixture_pin_count(catalog: &SqliteCatalog) -> Result<i64, Box<dyn Error>> {
        Ok(catalog
            .lock()?
            .query_row("SELECT COUNT(*) FROM native_pin_v1", [], |row| row.get(0))?)
    }

    #[test]
    fn selected_boot_manifest_survives_retention_and_explicit_retirement() -> TestResult {
        let root = tempfile::tempdir()?;
        let (catalog, genesis) = setup(root.path())?;
        let original = operation(13001, 151);
        let attempt = admit_new(&catalog, original)?;
        let stale_result = object(152, vec![]);
        reserve_confirm(&catalog, attempt, std::slice::from_ref(&stale_result))?;
        let first = view(1, genesis.profile, Some(stale_result.reference));
        check_eq!(
            publish(&catalog, attempt, &genesis, first.clone())?,
            OperationStatus::Published(first.identity)
        );
        let selected =
            catalog.select_native_boot_manifest(original.key, &mut admission_budget())?;
        check_eq!(selected.operation, original);
        check_eq!(selected.publication.source, first.source);
        // Independent SHA-256 of the source domain and fixed all-empty JSON.
        check_eq!(
            selected.source_cut_digest()?,
            Digest([
                0x23, 0x71, 0x15, 0x1a, 0x78, 0x07, 0x13, 0xd9, 0x8f, 0xe0, 0x72, 0xd4, 0xb1, 0x47,
                0x44, 0x42, 0xee, 0xeb, 0xf9, 0x32, 0x39, 0x2d, 0x4c, 0xd1, 0xb2, 0x64, 0xb8, 0x14,
                0x89, 0xcd, 0x83, 0x41,
            ])
        );
        let second = publish_plain_next(&catalog, &first, 2, 153)?;
        check_eq!(
            catalog.advance_exact_history_floor(workspace(), 7, 2, &mut admission_budget())?,
            2
        );
        let retired =
            catalog.retire_exact_history_page(workspace(), 7, None, 8, &mut admission_budget())?;
        check_eq!(retired.retired, 2);
        check_eq!(view_rows(&catalog, 1)?, 0);
        let released =
            catalog.release_terminal_custody(original.key, 256, &mut admission_budget())?;
        check!(!released.has_more);
        let direct = Connection::open(catalog.path())?;
        let saved: i64 = direct.query_row(
            "SELECT count(*) FROM native_operation_object_v1 WHERE operation=?1 AND object=?2",
            params![operation_key(original.key)?, object_key(original.request)],
            |row| row.get(0),
        )?;
        check_eq!(saved, 1);
        let results: i64 = direct.query_row(
            "SELECT count(*) FROM native_attempt_object_v1 WHERE operation=?1",
            params![operation_key(original.key)?],
            |row| row.get(0),
        )?;
        check_eq!(results, 0);
        drop(direct);
        drop(catalog);
        let catalog = open(root.path())?;
        check_eq!(
            catalog.native_boot_manifest(&mut admission_budget())?,
            Some(selected.clone())
        );
        // Original view rows may be gone; idempotent selection uses its durable
        // binding and does not reselect Latest or backfill a new source.
        check_eq!(
            catalog.select_native_boot_manifest(original.key, &mut admission_budget())?,
            selected
        );
        check_eq!(
            inspect_with_test_budget(&catalog, original.key)?,
            OperationStatus::Published(first.identity)
        );
        let next_operation = operation(922, 153);
        let replacement =
            catalog.select_native_boot_manifest(next_operation.key, &mut admission_budget())?;
        check_eq!(replacement.publication.identity, second.identity);
        check_eq!(
            catalog.native_boot_manifest(&mut admission_budget())?,
            Some(replacement)
        );
        let old = catalog.release_terminal_custody(original.key, 256, &mut admission_budget())?;
        check!(!old.has_more);
        let direct = Connection::open(catalog.path())?;
        check_eq!(
            direct.query_row(
                "SELECT count(*) FROM native_operation_object_v1 WHERE operation=?1",
                params![operation_key(original.key)?],
                |row| row.get::<_, i64>(0)
            )?,
            0
        );
        drop(direct);
        catalog.clear_native_boot_manifest(&mut admission_budget())?;
        check_eq!(catalog.native_boot_manifest(&mut admission_budget())?, None);
        check!(
            !catalog
                .release_terminal_custody(next_operation.key, 256, &mut admission_budget())?
                .has_more
        );
        let direct = Connection::open(catalog.path())?;
        check_eq!(
            direct.query_row(
                "SELECT count(*) FROM native_operation_object_v1 WHERE operation=?1",
                params![operation_key(next_operation.key)?],
                |row| row.get::<_, i64>(0)
            )?,
            0
        );
        Ok(())
    }

    #[test]
    fn selected_boot_manifest_refusals_preserve_selection_and_custody() -> TestResult {
        let root = tempfile::tempdir()?;
        let (catalog, genesis) = setup(root.path())?;
        let first_op = operation(13002, 154);
        let first_attempt = admit_new(&catalog, first_op)?;
        let first = view(1, genesis.profile, None);
        check_eq!(
            publish(&catalog, first_attempt, &genesis, first.clone())?,
            OperationStatus::Published(first.identity)
        );
        let selected =
            catalog.select_native_boot_manifest(first_op.key, &mut admission_budget())?;
        let pending = operation(13003, 155);
        let attempt = admit_new(&catalog, pending)?;
        check_eq!(
            catalog.select_native_boot_manifest(pending.key, &mut admission_budget()),
            Err(IndexError::Conflict)
        );
        check_eq!(
            catalog.refuse(attempt, Refusal::InvalidInput)?,
            OperationStatus::Refused(Refusal::InvalidInput)
        );
        check_eq!(
            catalog.select_native_boot_manifest(pending.key, &mut admission_budget()),
            Err(IndexError::Conflict)
        );
        let mut denied = WorkBudget::with_request(
            WorkLimits {
                reads: 100_000,
                writes: 0,
                bytes: 32 * 1024 * 1024,
                mutations: 0,
            },
            quanta_index_core::RequestBudgetV1::for_duration(Duration::from_secs(30)),
        );
        check_eq!(
            catalog.clear_native_boot_manifest(&mut denied),
            Err(IndexError::ResourceLimited)
        );
        check_eq!(
            catalog.native_boot_manifest(&mut admission_budget())?,
            Some(selected.clone())
        );
        let request = quanta_index_core::RequestBudgetV1::for_duration(Duration::from_secs(30));
        request.cancel_handle().cancel();
        let mut cancelled = WorkBudget::with_request(
            WorkLimits {
                reads: 100_000,
                writes: 100_000,
                bytes: 32 * 1024 * 1024,
                mutations: 100_000,
            },
            request,
        );
        check_eq!(
            catalog.clear_native_boot_manifest(&mut cancelled),
            Err(IndexError::Cancelled)
        );
        check_eq!(cancelled.usage().reads, 0);
        let direct = Connection::open(catalog.path())?;
        check_eq!(
            direct.execute(
                "DELETE FROM native_operation_object_v1 WHERE operation=?1 AND object=?2",
                params![operation_key(first_op.key)?, object_key(first_op.request)]
            )?,
            1
        );
        drop(direct);
        check!(matches!(
            catalog.native_boot_manifest(&mut admission_budget()),
            Err(IndexError::Storage(_))
        ));
        check!(matches!(
            catalog.select_native_boot_manifest(first_op.key, &mut admission_budget()),
            Err(IndexError::Storage(_))
        ));
        check!(matches!(
            catalog.clear_native_boot_manifest(&mut admission_budget()),
            Err(IndexError::Storage(_))
        ));
        // The GC orphan fallback also refuses this missing relation, rather
        // than authorizing deletion from absence alone.
        let guard = catalog.lock().map_err(|error| error.to_string())?;
        check!(matches!(
            boot_manifest::refuse_selected_orphan(
                &guard,
                first_op.request,
                &mut admission_budget()
            ),
            Err(IndexError::Storage(_))
        ));
        check_eq!(boot_manifest::read_stored(&guard)?, Some(selected));
        drop(guard);
        Ok(())
    }

    #[test]
    fn selected_boot_manifest_preserves_canonical_view_proof_and_fixed_size_bound() -> TestResult {
        let root = tempfile::tempdir()?;
        let (catalog, genesis) = setup(root.path())?;
        let op = operation(13005, 157);
        let attempt = admit_new(&catalog, op)?;
        let next = view(1, genesis.profile, None);
        check_eq!(
            publish(&catalog, attempt, &genesis, next.clone())?,
            OperationStatus::Published(next.identity)
        );
        let mut selected = catalog.select_native_boot_manifest(op.key, &mut admission_budget())?;
        let nonce = catalog.native_root_nonce(&mut admission_budget())?;
        // Even an independently recomputed selection checksum cannot substitute
        // profile metadata under the original published identity digest.
        selected.publication.profile = object(158, vec![]).reference;
        let payload = serde_json::to_vec(&selected)?;
        let mut hash = Sha256::new();
        hash.update(b"qidx.native.boot-manifest.v1\0");
        hash.update(nonce);
        hash.update(&payload);
        let checksum: [u8; 32] = hash.finalize().into();
        let direct = Connection::open(catalog.path())?;
        check_eq!(direct.execute("UPDATE native_capture_format_v1 SET boot_manifest=?1,boot_manifest_digest=?2 WHERE singleton=1",
            params![&payload,checksum.as_slice()])?,1);
        drop(direct);
        check!(matches!(
            catalog.native_boot_manifest(&mut admission_budget()),
            Err(IndexError::Storage(_))
        ));

        let mut widest = terminal_custody::widest_operation_key();
        widest.epoch = i64::MAX.unsigned_abs();
        let object = ObjectRef(Digest([255; 32]));
        let mut publication = view(i64::MAX.unsigned_abs(), object, Some(object));
        publication.identity.workspace = widest.workspace;
        publication.identity.epoch = widest.epoch;
        publication.source = SourceCut {
            owners: MapRoot(Some(object)),
            paths: MapRoot(Some(object)),
            frontiers: MapRoot(Some(object)),
        };
        publication.projections = MapRoot(Some(object));
        publication.coverage = MapRoot(Some(object));
        publication.identity.digest = publication.identity_digest()?;
        let manifest = NativeBootManifest {
            operation: ImmutableOperation {
                key: widest,
                request: object,
                source_mode: SourceOperationMode::Resync {
                    prior_epoch: u64::MAX,
                },
            },
            publication,
        };
        check!(serde_json::to_vec(&manifest)?.len() <= 4096);
        check_eq!(manifest.protected_roots(), [Some(object); 5]);
        Ok(())
    }

    #[test]
    fn selected_boot_manifest_corruption_refuses_without_rewriting_stored_bytes() -> TestResult {
        for mutation in [
            "UPDATE native_capture_format_v1 SET boot_manifest_digest=zeroblob(32)",
            "UPDATE native_capture_format_v1 SET boot_manifest=zeroblob(4097)",
            "UPDATE native_capture_format_v1 SET boot_manifest='text'",
            "UPDATE native_capture_format_v1 SET boot_manifest_digest=NULL",
        ] {
            let root = tempfile::tempdir()?;
            let (catalog, genesis) = setup(root.path())?;
            let op = operation(13004, 156);
            let attempt = admit_new(&catalog, op)?;
            let next = view(1, genesis.profile, None);
            check_eq!(
                publish(&catalog, attempt, &genesis, next.clone())?,
                OperationStatus::Published(next.identity)
            );
            let _selected = catalog.select_native_boot_manifest(op.key, &mut admission_budget())?;
            let database_path = catalog.path().to_path_buf();
            drop(catalog);
            let direct = Connection::open(&database_path)?;
            direct.execute_batch("PRAGMA ignore_check_constraints=ON")?;
            direct.execute_batch(mutation)?;
            let stored: (rusqlite::types::Value, rusqlite::types::Value) = direct.query_row(
                "SELECT boot_manifest,boot_manifest_digest FROM native_capture_format_v1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            drop(direct);
            check!(matches!(open(root.path()), Err(CoreError::Storage(_))));
            let direct = Connection::open(&database_path)?;
            check_eq!(
                direct.query_row(
                    "SELECT boot_manifest,boot_manifest_digest FROM native_capture_format_v1",
                    [],
                    |row| Ok((
                        row.get::<_, rusqlite::types::Value>(0)?,
                        row.get::<_, rusqlite::types::Value>(1)?
                    ))
                )?,
                stored
            );
        }
        Ok(())
    }
    fn fixture_read_ingress(
        catalog: &SqliteCatalog,
        operation: OperationKey,
    ) -> Result<Option<IngressRow>, IndexError> {
        let guard = catalog
            .lock()
            .map_err(|error| IndexError::Storage(format!("fixture ingress lock: {error}")))?;
        let ingress = read_ingress(&guard, operation);
        drop(guard);
        ingress
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
        publication: &mut PreparedPublication,
        actions: &[WorkAction],
    ) -> Result<(), IndexError> {
        let mut summary = WorkActionDigest::default();
        for action in actions {
            summary.push(action)?;
        }
        publication.work_actions = WorkActionSet {
            count: summary.count(),
            digest: summary.finish(),
        };
        Ok(())
    }

    fn commit_actions(
        catalog: &SqliteCatalog,
        mut publication: PreparedPublication,
        actions: &[WorkAction],
    ) -> Result<OperationStatus, IndexError> {
        stage_action_set(&mut publication, actions)?;
        let mut cursor = VecActions(actions.iter().copied().collect());
        let mut budget = WorkBudget::new(
            WorkLimits {
                reads: 4096,
                writes: 4096,
                bytes: 1 << 20,
                mutations: 4096,
            },
            Instant::now()
                .checked_add(Duration::from_secs(5))
                .ok_or(IndexError::ResourceLimited)?,
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
                source_mode: SourceOperationMode::Unchanged,
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
            (READY_PENDING_QUERY, "native_owned_open_idx_v1", 3),
            (READY_EXPIRED_QUERY, "native_owned_claim_idx_v1", 4),
        ] {
            let explain = format!("EXPLAIN QUERY PLAN {query}");
            let mut statement = connection.prepare(&explain)?;
            let rows = statement.query_map(
                rusqlite::params_from_iter((0..parameters).map(|_| 1_i64)),
                |row| row.get::<_, String>(3),
            )?;
            let plan = rows.collect::<Result<Vec<_>, _>>()?;
            check!(plan.iter().any(|detail| detail.contains(index)), "{plan:?}");
            check!(
                plan.iter().all(|detail| !detail.contains("TEMP B-TREE")),
                "{plan:?}"
            );
        }
        Ok(())
    }

    #[test]
    fn maximal_closed_work_row_stays_below_scan_bound() -> TestResult {
        let bytes16 = [u8::MAX; 16];
        let bytes32 = [u8::MAX; 32];
        let workspace = WorkspaceKey {
            tenant: TenantId(bytes16),
            collection: CollectionId(bytes16),
            worktree: WorktreeId(bytes16),
            slot: SlotId(bytes16),
        };
        let owner = OwnerKey {
            producer: ProducerId(bytes16),
            stable_id: StableOwnerId(bytes16),
        };
        let input = OwnerInputBinding {
            source: Some(ObjectRef(Digest(bytes32))),
            projection: Some(ObjectRef(Digest(bytes32))),
            producer_context: Digest(bytes32),
            derivation_policy: Digest(bytes32),
        };
        let created_operation = OperationKey {
            workspace,
            epoch: u64::MAX,
            producer: ProducerId(bytes16),
            stream: StreamId(bytes16),
            stream_epoch: u64::MAX,
            sequence: u64::MAX,
        };
        let created_view = ViewRef {
            workspace,
            epoch: u64::MAX,
            sequence: u64::MAX,
            digest: Digest(bytes32),
        };
        let prepared = OwnerMemberBinding {
            instance: IndexInstanceId(bytes16),
            owner,
            input,
            format: StorageFormatId(bytes16),
            artifact: ObjectRef(Digest(bytes32)),
            logical_commitment: Digest(bytes32),
        };
        let source = SourceCut {
            owners: MapRoot(Some(ObjectRef(Digest(bytes32)))),
            paths: MapRoot(Some(ObjectRef(Digest(bytes32)))),
            frontiers: MapRoot(Some(ObjectRef(Digest(bytes32)))),
        };
        let scopes = [
            WorkScope::Owner {
                owner,
                expected: input,
            },
            WorkScope::ProjectionDomain {
                projection: ProjectionId(bytes16),
                source,
                producer_policy: Digest(bytes32),
                dependency_context: Digest(bytes32),
            },
        ];
        for scope in scopes {
            let intent = WorkIntent {
                workspace,
                epoch: u64::MAX,
                instance: IndexInstanceId(bytes16),
                scope,
            };
            let row = WorkRow {
                intent,
                created_operation,
                created_view,
                status: WorkStatus::Claimed(WorkClaim {
                    intent,
                    fence: u64::MAX,
                    lease_until_ms: u64::MAX,
                }),
                ownership: WorkOwnership::Managed,
                ready_since_ms: u64::MAX,
                prepared: Some(prepared.clone()),
            };
            check!(
                encode(&row)?.len() <= usize::try_from(owned_work::MAX_OWNED_WORK_ROW_BYTES)?,
                "closed WorkRow exceeds durable scan bound"
            );
        }
        Ok(())
    }

    #[test]
    fn invalid_durable_work_key_is_rejected_before_scan_copy() -> TestResult {
        let root = tempfile::tempdir()?;
        let (catalog, genesis) = setup(root.path())?;
        let attempt = admit_new(&catalog, operation(205, 206))?;
        let intent = owner_work(96);
        let next = view(1, genesis.profile, None);
        let status = commit_actions(
            &catalog,
            PreparedPublication {
                attempt,
                expected: genesis.identity,
                next,
                work_actions: WorkActionSet::empty(),
                complete_work: None,
                source_events: vec![],
                projection_events: vec![],
                expected_source: None,
                source_mode: SourceOperationMode::Unchanged,
            },
            &[WorkAction::Create(intent)],
        )?;
        check!(matches!(status, OperationStatus::Published(_)));
        {
            let guard = catalog.lock()?;
            let changed = guard.execute(
                "UPDATE native_work_v1 SET work=zeroblob(33) WHERE work=?1",
                params![work_key(&intent)?],
            )?;
            check_eq!(changed, 1);
            drop(guard);
        }
        check!(matches!(
            catalog.ready_owned_work(workspace(), 7, 1, &mut admission_budget()),
            Err(IndexError::Storage(message)) if message.contains("native work blob length is not 32 bytes")
        ));
        check!(matches!(
            catalog.ready_work(workspace(), 7, 1, &mut admission_budget()),
            Err(IndexError::Storage(message)) if message.contains("native work blob length is not 32 bytes")
        ));
        Ok(())
    }

    #[test]
    fn oversized_durable_work_row_is_reported_before_payload_decode() -> TestResult {
        let root = tempfile::tempdir()?;
        let (catalog, genesis) = setup(root.path())?;
        let attempt = admit_new(&catalog, operation(203, 204))?;
        let intent = owner_work(95);
        let next = view(1, genesis.profile, None);
        let status = commit_actions(
            &catalog,
            PreparedPublication {
                attempt,
                expected: genesis.identity,
                next,
                work_actions: WorkActionSet::empty(),
                complete_work: None,
                source_events: vec![],
                projection_events: vec![],
                expected_source: None,
                source_mode: SourceOperationMode::Unchanged,
            },
            &[WorkAction::Create(intent)],
        )?;
        check!(matches!(status, OperationStatus::Published(_)));
        {
            let guard = catalog.lock()?;
            let changed = guard.execute(
                "UPDATE native_work_v1 SET payload=zeroblob(?2) WHERE work=?1",
                params![
                    work_key(&intent)?,
                    i64::try_from(
                        owned_work::MAX_OWNED_WORK_ROW_BYTES
                            .checked_add(1)
                            .ok_or(IndexError::ResourceLimited)?
                    )?
                ],
            )?;
            drop(guard);
            check_eq!(changed, 1);
        }
        check!(matches!(
            catalog.ready_owned_work(workspace(), 7, 1, &mut admission_budget()),
            Err(IndexError::Storage(message)) if message.contains("oversized owned work row")
        ));
        check!(matches!(
            catalog.ready_work(workspace(), 7, 1, &mut admission_budget()),
            Err(IndexError::Storage(message)) if message.contains("oversized owned work row")
        ));
        check!(matches!(
            catalog.work_status(&intent, &admission_budget()),
            Err(IndexError::Storage(message)) if message.contains("oversized owned work row")
        ));
        Ok(())
    }

    #[test]
    fn materialization_pin_is_atomic_and_does_not_extend_execution_claim() -> TestResult {
        let root = tempfile::tempdir()?;
        let clock = Arc::new(ManualClock::new(1_000));
        let (catalog, genesis) = setup_with_clock(root.path(), clock.clone())?;
        let intent = owner_work(93);
        let queued = publish_work_next(&catalog, &genesis, 1, &intent, 199, 200)?;
        let session = catalog.open_read_session()?;
        let assignment = catalog
            .acquire_owned_work(&intent, retirement_owned_policy(), &admission_budget())?
            .ok_or("missing owned assignment")?;
        let grant = require_owned_grant(
            catalog.begin_owned_work(&assignment)?,
            "missing owned grant",
        )?;
        let request = &grant.assignment.request;
        let UpdateKind::Materialize { claim } = &request.kind else {
            return Err("expected owned claim".into());
        };
        let latest = publish_plain_next(&catalog, &queued, 2, 201)?;
        check_eq!(request.expected_view, queued.identity);
        let before = {
            let connection = catalog.lock()?;
            let counter: i64 = connection.query_row(
                "SELECT next FROM native_counter_v1 WHERE name='attempt_fence'",
                [],
                |row| row.get(0),
            )?;
            connection.execute_batch(
                "CREATE TRIGGER fail_materialization_pin BEFORE UPDATE OF row_digest ON native_pin_v1
                 BEGIN SELECT RAISE(ABORT, 'pin seal fault'); END;",
            )?;
            counter
        };
        clock.set(1_010);
        check!(matches!(
            catalog.acquire_materialization_ticket(
                session.as_ref(),
                request,
                Duration::from_secs(60)
            ),
            Err(IndexError::Storage(_))
        ));
        {
            let connection = catalog.lock()?;
            let rows: i64 =
                connection.query_row("SELECT COUNT(*) FROM native_pin_v1", [], |row| row.get(0))?;
            let floor: i64 = connection.query_row(
                "SELECT last_ms FROM native_work_clock_v1 WHERE singleton=1",
                [],
                |row| row.get(0),
            )?;
            let counter: i64 = connection.query_row(
                "SELECT next FROM native_counter_v1 WHERE name='attempt_fence'",
                [],
                |row| row.get(0),
            )?;
            check_eq!((rows, floor, counter), (0, 1_000, before));
            connection.execute_batch("DROP TRIGGER fail_materialization_pin;")?;
        }
        let ticket = catalog.acquire_materialization_ticket(
            session.as_ref(),
            request,
            Duration::from_secs(60),
        )?;
        check_eq!(ticket.view, latest);
        check_eq!(ticket.lease.session, session.binding());
        check_eq!(ticket.lease.expires_at_ms, 61_010);
        check_eq!(
            catalog.work_status(&intent, &admission_budget())?,
            WorkStatus::Claimed(*claim)
        );
        catalog.validate_ticket(session.as_ref(), ticket.lease)?;
        check_eq!(catalog.prepared_work(claim)?, None);
        // A live read pin is not execution authority. The independent check
        // immediately before the provider still rejects an expired claim.
        clock.set(claim.lease_until_ms);
        catalog.validate_ticket(session.as_ref(), ticket.lease)?;
        check_eq!(catalog.prepared_work(claim), Err(IndexError::WorkFenceLost));
        check!(matches!(
            catalog.acquire_materialization_ticket(
                session.as_ref(),
                request,
                Duration::from_millis(50)
            ),
            Err(IndexError::WorkFenceLost)
        ));
        catalog.release_ticket(session.as_ref(), ticket.lease)?;
        let rows: i64 =
            catalog
                .lock()?
                .query_row("SELECT COUNT(*) FROM native_pin_v1", [], |row| row.get(0))?;
        check_eq!(rows, 0);
        Ok(())
    }

    #[test]
    fn materialization_pin_rejects_altered_owned_body_and_foreign_session() -> TestResult {
        let root = tempfile::tempdir()?;
        let (catalog, genesis) = setup(root.path())?;
        let intent = owner_work(91);
        let _queued = publish_work_next(&catalog, &genesis, 1, &intent, 197, 198)?;
        let session = catalog.open_read_session()?;
        let assignment = catalog
            .acquire_owned_work(&intent, retirement_owned_policy(), &admission_budget())?
            .ok_or("missing owned assignment")?;
        let grant = require_owned_grant(
            catalog.begin_owned_work(&assignment)?,
            "missing owned grant",
        )?;
        let mut altered = grant.assignment.request.clone();
        altered.expected_view.epoch += 1;
        check!(matches!(
            catalog.acquire_materialization_ticket(
                session.as_ref(),
                &altered,
                Duration::from_secs(1)
            ),
            Err(IndexError::DigestConflict)
        ));
        let foreign_root = tempfile::tempdir()?;
        let (foreign, _) = setup(foreign_root.path())?;
        let foreign_session = foreign.open_read_session()?;
        check!(
            catalog
                .acquire_materialization_ticket(
                    foreign_session.as_ref(),
                    &grant.assignment.request,
                    Duration::from_secs(1)
                )
                .is_err()
        );
        let rows: i64 =
            catalog
                .lock()?
                .query_row("SELECT COUNT(*) FROM native_pin_v1", [], |row| row.get(0))?;
        check_eq!(rows, 0);
        Ok(())
    }

    #[test]
    fn owned_materialization_pin_and_completion_read_each_live_row_once() -> TestResult {
        let root = tempfile::tempdir()?;
        let clock = Arc::new(ManualClock::new(1_000));
        let (catalog, genesis) = setup_with_clock(root.path(), clock)?;
        let intent = owner_work(93);
        let queued = publish_work_next(&catalog, &genesis, 1, &intent, 199, 200)?;
        let session = catalog.open_read_session()?;
        let assignment = catalog
            .acquire_owned_work(&intent, retirement_owned_policy(), &admission_budget())?
            .ok_or("missing owned assignment")?;
        let grant = require_owned_grant(
            catalog.begin_owned_work(&assignment)?,
            "missing owned grant",
        )?;
        let request = &grant.assignment.request;
        let UpdateKind::Materialize { claim } = &request.kind else {
            return Err("owned request lost materialization claim".into());
        };
        let claim = *claim;
        let (before_admission, before_reads) =
            counted_owned_materialization(&catalog, || catalog.validate_owned_work(request))?;
        check_eq!(before_admission, OperationStatus::Unknown);
        check_eq!(before_reads, (1, 1, 2));
        check_eq!(OWNED_OPERATION_READS.with(std::cell::Cell::get), 1);

        let (ticket, pin_reads) = counted_owned_materialization(&catalog, || {
            catalog.acquire_materialization_ticket(
                session.as_ref(),
                request,
                Duration::from_secs(1),
            )
        })?;
        check_eq!(pin_reads, (1, 1, 2));
        check_eq!(ticket.view, queued);
        check_eq!(ticket.lease.session, session.binding());
        check_eq!(fixture_pin_count(&catalog)?, 1);
        catalog.validate_ticket(session.as_ref(), ticket.lease)?;
        catalog.release_ticket(session.as_ref(), ticket.lease)?;
        check_eq!(fixture_pin_count(&catalog)?, 0);

        let request_object = object(180, vec![]);
        let immutable = ImmutableOperation {
            key: request.operation,
            request: request_object.reference,
            source_mode: SourceOperationMode::Unchanged,
        };
        let ingress = stage_inputs(&catalog, immutable, std::slice::from_ref(&request_object))?;
        let attempt = match catalog.admit_common(
            immutable,
            ingress,
            &[],
            Some(request),
            &mut admission_budget(),
        )? {
            AdmissionOutcome::New(attempt) => attempt,
            AdmissionOutcome::Existing(status) => {
                return Err(format!("expected fresh owned admission, got {status:?}").into());
            }
        };
        let (admitted, admission_reads) =
            counted_owned_materialization(&catalog, || catalog.validate_owned_work(request))?;
        check_eq!(admitted, OperationStatus::Pending(attempt));
        check_eq!(admission_reads, (1, 1, 2));
        check_eq!(OWNED_OPERATION_READS.with(std::cell::Cell::get), 1);
        let artifact = object(181, vec![]);
        reserve_confirm(&catalog, attempt, std::slice::from_ref(&artifact))?;
        catalog.stage_work(&claim, owner_result(&intent, artifact.reference)?)?;
        let mut completed = view(2, genesis.profile, None);
        completed.coverage = MapRoot(Some(artifact.reference));
        completed.identity.digest = completed.identity_digest()?;
        let (status, publication_reads) = counted_owned_materialization(&catalog, || {
            commit_empty(
                &catalog,
                PreparedPublication {
                    attempt,
                    source_mode: SourceOperationMode::Unchanged,
                    expected: queued.identity,
                    next: completed.clone(),
                    work_actions: WorkActionSet::empty(),
                    complete_work: Some(claim),
                    source_events: vec![],
                    projection_events: vec![],
                    expected_source: None,
                },
            )
        })?;
        check_eq!(publication_reads, (1, 1, 2));
        check_eq!(status, OperationStatus::Published(completed.identity));
        let (terminal, _terminal_reads) =
            counted_owned_materialization(&catalog, || catalog.validate_owned_work(request))?;
        check_eq!(terminal, OperationStatus::Published(completed.identity));
        check_eq!(OWNED_OPERATION_READS.with(std::cell::Cell::get), 1);
        let mut altered = request.clone();
        altered.expected_view.sequence = altered
            .expected_view
            .sequence
            .checked_add(1)
            .ok_or("owned terminal view sequence overflow")?;
        check_eq!(
            catalog.validate_owned_work(&altered),
            Err(IndexError::DigestConflict)
        );
        check_eq!(fixture_current_view(&catalog)?, completed);
        check_eq!(
            catalog.work_status(&intent, &admission_budget())?,
            WorkStatus::Completed(completed.identity)
        );
        check_eq!(
            inspect_with_test_budget(&catalog, request.operation)?,
            OperationStatus::Published(completed.identity)
        );
        let guard = catalog.lock()?;
        let parked = owned_work::read_owned(&guard, &intent)?.ok_or("lost owned assignment")?;
        check!(!parked.active);
        check_eq!(parked.due_ms, u64::try_from(i64::MAX)?);
        drop(guard);
        check_eq!(
            catalog.acquire_materialization_ticket(
                session.as_ref(),
                request,
                Duration::from_secs(1),
            ),
            Err(IndexError::WorkFenceLost)
        );
        check_eq!(fixture_pin_count(&catalog)?, 0);
        check_eq!(fixture_current_view(&catalog)?, completed);

        // Published is terminal: the original fallback checks the completed
        // work before consulting the now-irrelevant managed assignment row.
        let guard = catalog.lock()?;
        check_eq!(
            guard.execute(
                "UPDATE native_owned_work_v1 SET row_digest=zeroblob(32) WHERE work=?1",
                params![work_key(&intent)?],
            )?,
            1
        );
        drop(guard);
        check_eq!(
            catalog.acquire_materialization_ticket(
                session.as_ref(),
                request,
                Duration::from_secs(1),
            ),
            Err(IndexError::WorkFenceLost)
        );
        check_eq!(fixture_pin_count(&catalog)?, 0);
        check_eq!(fixture_current_view(&catalog)?, completed);
        Ok(())
    }

    fn owned_unsealed_ingress(
        catalog: &SqliteCatalog,
        genesis: &CatalogView,
    ) -> Result<
        (
            UpdateRequest,
            ImmutableOperation,
            IngressRef,
            RegisteredObject,
        ),
        Box<dyn Error>,
    > {
        let intent = owner_work(90);
        let _queued = publish_work_next(catalog, genesis, 1, &intent, 199, 200)?;
        let assignment = catalog
            .acquire_owned_work(&intent, retirement_owned_policy(), &admission_budget())?
            .ok_or("missing owned assignment")?;
        let grant = require_owned_grant(
            catalog.begin_owned_work(&assignment)?,
            "missing owned grant",
        )?;
        // These Catalog transaction tests use registered fixture DATA. The real
        // Kernel lifecycle test independently checks the pure identity port and
        // the original immutable request codec/physical object store.
        let registration = object(203, vec![]);
        let operation = ImmutableOperation {
            key: grant.assignment.request.operation,
            request: registration.reference,
            source_mode: SourceOperationMode::Unchanged,
        };
        let ingress = catalog.open_ingress(operation.key)?;
        catalog.reserve_inputs(ingress, std::slice::from_ref(&registration))?;
        Ok((grant.assignment.request, operation, ingress, registration))
    }

    #[test]
    fn owned_seal_admission_rolls_back_both_boundaries_and_recovers_original_receipt() -> TestResult
    {
        for fault in [
            "CREATE TRIGGER fail_owned_seal BEFORE UPDATE ON native_ingress_v1
             BEGIN SELECT RAISE(ABORT, 'owned seal fault'); END;",
            "CREATE TRIGGER fail_owned_seal BEFORE DELETE ON native_ingress_v1
             BEGIN SELECT RAISE(ABORT, 'owned admission deletion fault'); END;",
        ] {
            let root = tempfile::tempdir()?;
            let clock = Arc::new(ManualClock::new(1_000));
            let (catalog, genesis) = setup_with_clock(root.path(), clock.clone())?;
            let (request, operation, ingress, registration) =
                owned_unsealed_ingress(&catalog, &genesis)?;
            catalog.confirm_inputs(ingress, std::slice::from_ref(&registration))?;
            let before_ingress = fixture_read_ingress(&catalog, operation.key)?;
            check_eq!(
                before_ingress,
                Some(IngressRow {
                    stage: ingress,
                    sealed: None
                })
            );
            let before_clock_and_fence: (i64, i64) = {
                let connection = catalog.lock()?;
                let values = connection.query_row(
                    "SELECT last_ms,(SELECT next FROM native_counter_v1 WHERE name='attempt_fence')
                     FROM native_work_clock_v1 WHERE singleton=1",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )?;
                connection.execute_batch(fault)?;
                values
            };
            clock.set(1_010);
            check!(matches!(
                catalog.admit_common(
                    operation,
                    ingress,
                    &[],
                    Some(&request),
                    &mut admission_budget()
                ),
                Err(IndexError::Storage(_))
            ));
            check_eq!(
                inspect_with_test_budget(&catalog, operation.key)?,
                OperationStatus::Unknown
            );
            check_eq!(
                fixture_read_ingress(&catalog, operation.key)?,
                before_ingress
            );
            {
                let connection = catalog.lock()?;
                let after: (i64, i64) = connection.query_row(
                    "SELECT last_ms,(SELECT next FROM native_counter_v1 WHERE name='attempt_fence')
                     FROM native_work_clock_v1 WHERE singleton=1",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )?;
                check_eq!(after, before_clock_and_fence);
                check!(confirmed_ingress_object(
                    &connection,
                    operation.key,
                    registration.reference
                )?);
                let transferred: i64 = connection.query_row(
                    "SELECT COUNT(*) FROM native_operation_object_v1 WHERE operation=?1",
                    params![operation_key(operation.key)?],
                    |row| row.get(0),
                )?;
                check_eq!(transferred, 0);
                connection.execute_batch("DROP TRIGGER fail_owned_seal;")?;
            }
            let AdmissionOutcome::New(attempt) = catalog.admit_common(
                operation,
                ingress,
                &[],
                Some(&request),
                &mut admission_budget(),
            )?
            else {
                return Err("owned admission did not create its original attempt".into());
            };
            check_eq!(fixture_read_ingress(&catalog, operation.key)?, None);
            drop(catalog);
            let catalog =
                SqliteCatalog::open_with_clock(root.path(), Duration::from_secs(1), clock)?;
            check_eq!(catalog.operation(operation.key)?, operation);
            check_eq!(
                inspect_with_test_budget(&catalog, operation.key)?,
                OperationStatus::Pending(attempt)
            );
            // Replaying an existing original receipt needs no recreated ingress.
            check_eq!(
                catalog.admit_common(
                    operation,
                    replay_ingress(operation),
                    &[],
                    Some(&request),
                    &mut admission_budget()
                )?,
                AdmissionOutcome::Existing(OperationStatus::Pending(attempt))
            );
        }
        Ok(())
    }

    #[test]
    fn owned_seal_admission_preserves_unconfirmed_fence_identity_and_expiry_rejections()
    -> TestResult {
        for scenario in [
            "unconfirmed",
            "stale",
            "sealed_other",
            "expired",
            "altered",
            "cancelled",
        ] {
            let root = tempfile::tempdir()?;
            let clock = Arc::new(ManualClock::new(1_000));
            let (catalog, genesis) = setup_with_clock(root.path(), clock.clone())?;
            let (mut request, operation, ingress, registration) =
                owned_unsealed_ingress(&catalog, &genesis)?;
            if scenario != "unconfirmed" {
                catalog.confirm_inputs(ingress, std::slice::from_ref(&registration))?;
            }
            let mut selected = ingress;
            let mut budget = admission_budget();
            match scenario {
                "stale" => selected.fence += 1,
                "sealed_other" => {
                    let other = object(204, vec![]);
                    catalog.reserve_inputs(ingress, std::slice::from_ref(&other))?;
                    catalog.confirm_inputs(ingress, std::slice::from_ref(&other))?;
                    catalog.seal_inputs(ingress, other.reference)?;
                }
                "expired" => {
                    let UpdateKind::Materialize { claim } = &request.kind else {
                        return Err("owned request lost claim".into());
                    };
                    clock.set(claim.lease_until_ms);
                }
                "altered" => request.expected_view.sequence += 1,
                "cancelled" => budget.request().cancel_handle().cancel(),
                _ => {}
            }
            let before = fixture_read_ingress(&catalog, operation.key)?;
            let result =
                catalog.admit_common(operation, selected, &[], Some(&request), &mut budget);
            match scenario {
                "unconfirmed" => check!(matches!(result, Err(IndexError::InvalidInput(_)))),
                "stale" => check_eq!(result, Err(IndexError::FenceLost)),
                "sealed_other" | "altered" => check_eq!(result, Err(IndexError::DigestConflict)),
                "expired" => check_eq!(result, Err(IndexError::WorkFenceLost)),
                "cancelled" => check_eq!(result, Err(IndexError::Cancelled)),
                _ => return Err("unknown admission scenario".into()),
            }
            check_eq!(
                inspect_with_test_budget(&catalog, operation.key)?,
                OperationStatus::Unknown
            );
            check_eq!(fixture_read_ingress(&catalog, operation.key)?, before);
        }
        Ok(())
    }

    #[test]
    fn owned_begin_refreshes_lease_after_queue_wait() -> TestResult {
        use quanta_index_core::index::OwnedWorkPolicy;

        let root = tempfile::tempdir()?;
        let clock = Arc::new(ManualClock::new(1_000));
        let (catalog, genesis) = setup_with_clock(root.path(), clock.clone())?;
        let attempt = admit_new(&catalog, operation(201, 202))?;
        let intent = owner_work(94);
        let next = view(1, genesis.profile, None);
        let status = commit_actions(
            &catalog,
            PreparedPublication {
                attempt,
                expected: genesis.identity,
                next,
                work_actions: WorkActionSet::empty(),
                complete_work: None,
                source_events: vec![],
                projection_events: vec![],
                expected_source: None,
                source_mode: SourceOperationMode::Unchanged,
            },
            &[WorkAction::Create(intent)],
        )?;
        check!(matches!(status, OperationStatus::Published(_)));
        let policy = OwnedWorkPolicy {
            dispatch_limits: WorkLimits {
                reads: 100,
                writes: 100,
                bytes: 4096,
                mutations: 100,
            },
            max_dispatches: 2,
            total_timeout_ms: 1_000,
            lease_ms: 30,
            retry_backoff_ms: 1,
        };
        let assignment = catalog
            .acquire_owned_work(&intent, policy, &admission_budget())?
            .ok_or("missing assignment")?;
        let WorkStatus::Claimed(before) = catalog.work_status(&intent, &admission_budget())? else {
            return Err("expected original claim".into());
        };
        check_eq!(before.lease_until_ms, 1_030);
        // The grant starts just before the original lease ends.
        clock.set(1_029);
        let grant = require_owned_grant(catalog.begin_owned_work(&assignment)?, "missing grant")?;
        let WorkStatus::Claimed(after) = catalog.work_status(&intent, &admission_budget())? else {
            return Err("expected refreshed claim".into());
        };
        check_eq!(grant.assignment.request, assignment.request);
        check_eq!(grant.assignment.deadline_ms, assignment.deadline_ms);
        check_eq!(after.fence, before.fence);
        check_eq!(after.lease_until_ms, 1_059);
        clock.set(1_031);
        catalog.validate_materialization(&grant.assignment.request)?;
        check_eq!(
            catalog.validate_owned_work(&grant.assignment.request)?,
            OperationStatus::Unknown
        );
        let floor: i64 = catalog.lock()?.query_row(
            "SELECT last_ms FROM native_work_clock_v1 WHERE singleton=1",
            [],
            |row| row.get(0),
        )?;
        check_eq!(floor, 1_031);
        clock.set(1_030);
        check!(matches!(
            catalog.validate_materialization(&grant.assignment.request),
            Err(IndexError::Storage(reason)) if reason == "catalog clock regressed"
        ));
        check!(matches!(
            catalog.validate_owned_work(&grant.assignment.request),
            Err(IndexError::Storage(reason)) if reason == "catalog clock regressed"
        ));
        clock.set(1_059);
        check_eq!(
            catalog.validate_owned_work(&grant.assignment.request),
            Err(IndexError::WorkFenceLost)
        );
        Ok(())
    }

    #[test]
    fn owned_validation_preserves_clock_body_and_expiry_error_order() -> TestResult {
        let root = tempfile::tempdir()?;
        let clock = Arc::new(ManualClock::new(1_000));
        let (catalog, genesis) = setup_with_clock(root.path(), clock.clone())?;
        let (_intent, grant, completed) = owned_finish_fixture(&catalog, &genesis, false)?;
        check_eq!(completed, None);
        let request = &grant.assignment.request;
        let UpdateKind::Materialize { claim } = &request.kind else {
            return Err("owned fixture lost materialization claim".into());
        };
        let mut altered = request.clone();
        altered.expected_view.sequence = altered
            .expected_view
            .sequence
            .checked_add(1)
            .ok_or("owned view sequence overflow")?;
        clock.set(claim.lease_until_ms);
        check_eq!(
            catalog.validate_owned_work(&altered),
            Err(IndexError::DigestConflict)
        );
        check_eq!(
            catalog.validate_owned_work(request),
            Err(IndexError::WorkFenceLost)
        );
        clock.set(999);
        check!(matches!(
            catalog.validate_owned_work(&altered),
            Err(IndexError::Storage(reason)) if reason == "catalog clock regressed"
        ));
        clock.set(1_000);
        let changed = catalog.lock()?.execute(
            "UPDATE native_work_clock_v1 SET row_digest=X'00' WHERE singleton=1",
            [],
        )?;
        check_eq!(changed, 1);
        check!(matches!(
            catalog.validate_owned_work(&altered),
            Err(IndexError::Storage(reason)) if reason == "native work clock digest mismatch"
        ));
        Ok(())
    }

    #[test]
    fn owned_renewal_caps_durable_lease_and_rejects_stale_grant() -> TestResult {
        let root = tempfile::tempdir()?;
        let clock = Arc::new(ManualClock::new(1_000));
        let (catalog, genesis) = setup_with_clock(root.path(), clock.clone())?;
        let intent = owner_work(93);
        let _queued = publish_work_next(&catalog, &genesis, 1, &intent, 12_093, 93)?;
        let policy = OwnedWorkPolicy {
            dispatch_limits: WorkLimits {
                reads: 100,
                writes: 100,
                bytes: 4096,
                mutations: 100,
            },
            max_dispatches: 2,
            total_timeout_ms: 1_000,
            lease_ms: 900,
            retry_backoff_ms: 1,
        };
        let assignment = catalog
            .acquire_owned_work(&intent, policy, &admission_budget())?
            .ok_or("missing assignment")?;
        let grant = require_owned_grant(catalog.begin_owned_work(&assignment)?, "missing grant")?;
        clock.set(1_850);
        OWNED_RENEW_READS.with(|counter| counter.set((0, 0)));
        let guard = catalog.lock()?;
        guard.trace_v2(
            rusqlite::trace::TraceEventCodes::SQLITE_TRACE_STMT,
            Some(observe_owned_renew_reads),
        );
        drop(guard);
        let result = catalog.renew_owned_work(&grant);
        let guard = catalog.lock()?;
        guard.trace_v2(rusqlite::trace::TraceEventCodes::empty(), None);
        drop(guard);
        let renewed = result?;
        check_eq!(OWNED_RENEW_READS.with(std::cell::Cell::get), (1, 1));
        let UpdateKind::Materialize { claim } = &grant.assignment.request.kind else {
            return Err("owned grant is not materialization".into());
        };
        check_eq!(renewed.fence, claim.fence);
        check_eq!(renewed.lease_until_ms, assignment.deadline_ms);
        check_eq!(
            catalog.work_status(&intent, &admission_budget())?,
            WorkStatus::Claimed(renewed)
        );
        let (persisted, floor) = {
            let guard = catalog.lock()?;
            let persisted = owned_work::read_owned(&guard, &intent)?.ok_or("missing owned row")?;
            let floor: i64 = guard.query_row(
                "SELECT last_ms FROM native_work_clock_v1 WHERE singleton=1",
                [],
                |row| row.get(0),
            )?;
            drop(guard);
            (persisted, floor)
        };
        check_eq!(floor, 1_850);
        check!(persisted.active);
        check_eq!(persisted.dispatches, grant.ordinal);
        check_eq!(persisted.due_ms, assignment.deadline_ms);

        let mut wrong = grant.clone();
        wrong.ordinal = wrong.ordinal.checked_add(1).ok_or("ordinal overflow")?;
        check_eq!(
            catalog.renew_owned_work(&wrong),
            Err(IndexError::WorkFenceLost)
        );
        clock.set(1_849);
        check!(matches!(
            catalog.renew_owned_work(&grant),
            Err(IndexError::Storage(reason)) if reason == "catalog clock regressed"
        ));
        clock.set(1_850);
        check_eq!(
            catalog.work_status(&intent, &admission_budget())?,
            WorkStatus::Claimed(renewed)
        );
        let guard = catalog.lock()?;
        check_eq!(
            owned_work::read_owned(&guard, &intent)?.ok_or("lost owned row")?,
            persisted
        );
        Ok(())
    }

    #[test]
    fn owned_work_freezes_policy_and_fences_expired_grants() -> TestResult {
        use quanta_index_core::index::OwnedWorkPolicy;

        let root = tempfile::tempdir()?;
        let clock = Arc::new(ManualClock::new(1_000));
        let (catalog, genesis) = setup_with_clock(root.path(), clock.clone())?;
        let operation = operation(244, 245);
        let attempt = admit_new(&catalog, operation)?;
        let intent = owner_work(92);
        let next = view(1, genesis.profile, None);
        check_eq!(
            commit_actions(
                &catalog,
                PreparedPublication {
                    attempt,
                    expected: genesis.identity,
                    next: next.clone(),
                    work_actions: WorkActionSet::empty(),
                    complete_work: None,
                    source_events: vec![],
                    projection_events: vec![],
                    expected_source: None,
                    source_mode: SourceOperationMode::Unchanged,
                },
                &[WorkAction::Create(intent)],
            )?,
            OperationStatus::Published(next.identity)
        );
        let limits = WorkLimits {
            reads: 100,
            writes: 100,
            bytes: 4096,
            mutations: 100,
        };
        let policy = OwnedWorkPolicy {
            dispatch_limits: limits,
            max_dispatches: 2,
            total_timeout_ms: 100,
            lease_ms: 20,
            retry_backoff_ms: 1,
        };
        check_eq!(
            catalog.ready_owned_work(workspace(), 7, 1, &mut admission_budget())?,
            OwnedReadyOutcome::Ready(vec![intent])
        );
        let assigned = catalog
            .acquire_owned_work(&intent, policy, &admission_budget())?
            .ok_or("missing assignment")?;
        check_eq!(assigned.policy, policy);
        let first =
            require_owned_grant(catalog.begin_owned_work(&assigned)?, "missing first grant")?;
        check_eq!(first.ordinal, 1);
        check_eq!(
            catalog.validate_owned_work(&first.assignment.request)?,
            OperationStatus::Unknown
        );
        catalog.validate_materialization(&first.assignment.request)?;
        let mut altered = first.assignment.request.clone();
        altered.expected_view.sequence += 1;
        check_eq!(
            catalog.validate_materialization(&altered),
            Err(IndexError::DigestConflict)
        );
        let mut altered = first.assignment.request.clone();
        let UpdateKind::Materialize { claim } = &mut altered.kind else {
            return Err("expected owned materialization".into());
        };
        claim.lease_until_ms += 1;
        check_eq!(
            catalog.validate_materialization(&altered),
            Err(IndexError::DigestConflict)
        );
        let mut external = first.assignment.request.clone();
        external.operation = operation.key;
        check_eq!(
            catalog.validate_materialization(&external),
            Err(IndexError::WorkFenceLost)
        );
        check_eq!(
            catalog.begin_owned_work(&assigned)?,
            OwnedWorkBegin::Deferred(quanta_index_core::index::OwnedWorkDefer::Active)
        );
        check!(
            catalog
                .open_ingress(first.assignment.request.operation)
                .is_ok()
        );
        check!(matches!(
            catalog.admit(
                ImmutableOperation {
                    key: first.assignment.request.operation,
                    request: ObjectRef(digest_id(246)),
                    source_mode: SourceOperationMode::Unchanged,
                },
                IngressRef {
                    operation: first.assignment.request.operation,
                    fence: 0
                },
                &[],
                &mut admission_budget(),
            ),
            Err(IndexError::InvalidInput(_))
        ));
        catalog.finish_owned_work(&first, OwnedWorkReport::Interrupted)?;
        let mut changed_policy = policy;
        changed_policy.max_dispatches = 3;
        let retained = catalog
            .acquire_owned_work(&intent, changed_policy, &admission_budget())?
            .ok_or("missing retained assignment")?;
        check_eq!(retained.policy, policy);
        check_eq!(retained.deadline_ms, assigned.deadline_ms);
        check_eq!(retained.dispatches, 1);
        clock.set(1_001);
        drop(catalog);
        let catalog =
            SqliteCatalog::open_with_clock(root.path(), Duration::from_secs(1), clock.clone())?;
        let retained = catalog
            .acquire_owned_work(&intent, changed_policy, &admission_budget())?
            .ok_or("missing reopened assignment")?;
        let second =
            require_owned_grant(catalog.begin_owned_work(&retained)?, "missing second grant")?;
        check_eq!(second.ordinal, 2);
        check_eq!(
            second.assignment.request.operation,
            first.assignment.request.operation
        );
        check_eq!(
            catalog.finish_owned_work(&first, OwnedWorkReport::Interrupted),
            Err(IndexError::WorkFenceLost)
        );
        clock.set(1_021);
        let rotated = catalog.acquire_owned_work(&intent, changed_policy, &admission_budget())?;
        check!(rotated.is_none());
        check_eq!(
            catalog.work_status(&intent, &admission_budget())?,
            WorkStatus::Blocked(Refusal::ResourceLimited)
        );
        check_eq!(
            catalog.validate_materialization(&second.assignment.request),
            Err(IndexError::WorkFenceLost)
        );
        check_eq!(
            catalog.ready_owned_work(workspace(), 7, 1, &mut admission_budget())?,
            OwnedReadyOutcome::Ready(vec![])
        );
        Ok(())
    }

    #[test]
    fn owned_begin_preserves_terminal_and_rejects_malformed_pending() -> TestResult {
        let root = tempfile::tempdir()?;
        let clock = Arc::new(ManualClock::new(1_000));
        let (catalog, genesis) = setup_with_clock(root.path(), clock.clone())?;
        let intent = owner_work(93);
        let _published = publish_work_next(&catalog, &genesis, 1, &intent, 12006, 160)?;
        let policy = OwnedWorkPolicy {
            dispatch_limits: WorkLimits {
                reads: 100,
                writes: 100,
                bytes: 4096,
                mutations: 100,
            },
            max_dispatches: 2,
            total_timeout_ms: 30_000,
            lease_ms: 2_000,
            retry_backoff_ms: 1,
        };
        let assigned = catalog
            .acquire_owned_work(&intent, policy, &admission_budget())?
            .ok_or("missing assignment")?;
        check_eq!(
            inspect_with_test_budget(&catalog, assigned.request.operation)?,
            OperationStatus::Unknown
        );
        clock.set(assigned.deadline_ms);
        check_eq!(
            catalog.begin_owned_work(&assigned)?,
            OwnedWorkBegin::Terminal(OwnedWorkTerminal::Blocked(Refusal::ResourceLimited))
        );
        check_eq!(
            catalog.work_status(&intent, &admission_budget())?,
            WorkStatus::Blocked(Refusal::ResourceLimited)
        );
        check_eq!(
            inspect_with_test_budget(&catalog, assigned.request.operation)?,
            OperationStatus::Unknown
        );
        set_terminal_work_status(&catalog, &intent, &WorkStatus::Pending)?;
        check!(matches!(
            catalog.begin_owned_work(&assigned),
            Err(IndexError::Storage(reason)) if reason == "managed work lost claim"
        ));
        Ok(())
    }

    #[test]
    fn owned_due_retry_precedes_older_unstarted_backlog_without_resetting_deadline() -> TestResult {
        for expired in [false, true] {
            let root = tempfile::tempdir()?;
            let clock = Arc::new(ManualClock::new(1_000));
            let (catalog, genesis) = setup_with_clock(root.path(), clock.clone())?;
            let attempt = admit_new(&catalog, operation(242, 243))?;
            let next = view(1, genesis.profile, None);
            let intents = [owner_work(80), owner_work(81), owner_work(82)];
            let mut actions = intents
                .into_iter()
                .map(|intent| {
                    let action = WorkAction::Create(intent);
                    action.key().map(|key| (key, action))
                })
                .collect::<Result<Vec<_>, IndexError>>()?;
            actions.sort_unstable_by_key(|(key, _action)| *key);
            let actions: Vec<_> = actions.into_iter().map(|(_key, action)| action).collect();
            check_eq!(
                commit_actions(
                    &catalog,
                    PreparedPublication {
                        attempt,
                        expected: genesis.identity,
                        next: next.clone(),
                        work_actions: WorkActionSet::empty(),
                        complete_work: None,
                        source_events: vec![],
                        projection_events: vec![],
                        expected_source: None,
                        source_mode: SourceOperationMode::Unchanged,
                    },
                    &actions,
                )?,
                OperationStatus::Published(next.identity)
            );
            let original = require_owned_ready(catalog.ready_owned_work(
                workspace(),
                7,
                3,
                &mut admission_budget(),
            )?)?;
            check_eq!(original.len(), 3);
            let [first, second, retry]: [_; 3] = original
                .as_slice()
                .try_into()
                .map_err(|_error| "expected three owned-work fixtures")?;
            let policy = quanta_index_core::index::OwnedWorkPolicy {
                dispatch_limits: WorkLimits {
                    reads: 100,
                    writes: 100,
                    bytes: 4096,
                    mutations: 100,
                },
                max_dispatches: 2,
                total_timeout_ms: 100,
                lease_ms: 30,
                retry_backoff_ms: 5,
            };
            let assignment = catalog
                .acquire_owned_work(&retry, policy, &admission_budget())?
                .ok_or("missing assignment")?;
            let grant = require_owned_grant(
                catalog.begin_owned_work(&assignment)?,
                "missing first grant",
            )?;
            catalog.finish_owned_work(&grant, OwnedWorkReport::Interrupted)?;
            // Older unstarted rows precede this retry's due timestamp. Their
            // backlog must not consume its already-running lifetime budget.
            clock.set(1_004);
            let waiting = require_owned_ready(catalog.ready_owned_work(
                workspace(),
                7,
                3,
                &mut admission_budget(),
            )?)?;
            check_eq!(waiting, vec![first, second]);
            drop(catalog);
            let catalog =
                SqliteCatalog::open_with_clock(root.path(), Duration::from_secs(1), clock.clone())?;
            clock.set(if expired { 1_101 } else { 1_005 });
            let changes = catalog.lock()?.total_changes();
            check_eq!(
                catalog.ready_owned_work(workspace(), 7, 0, &mut admission_budget())?,
                OwnedReadyOutcome::Ready(vec![])
            );
            check_eq!(
                catalog.ready_owned_work(workspace(), 7, 1, &mut admission_budget())?,
                OwnedReadyOutcome::Ready(vec![retry])
            );
            check_eq!(
                catalog.ready_owned_work(workspace(), 7, 3, &mut admission_budget())?,
                OwnedReadyOutcome::Ready(vec![retry, first, second])
            );
            let after = catalog.lock()?.total_changes();
            check_eq!(after, changes);
            let resumed = catalog.acquire_owned_work(&retry, policy, &admission_budget())?;
            if expired {
                check!(resumed.is_none());
                check_eq!(
                    catalog.work_status(&retry, &admission_budget())?,
                    WorkStatus::Blocked(Refusal::ResourceLimited)
                );
                check_eq!(
                    catalog.ready_owned_work(workspace(), 7, 3, &mut admission_budget())?,
                    OwnedReadyOutcome::Ready(vec![first, second])
                );
            } else {
                let resumed = resumed.ok_or("due retry was not admitted")?;
                check_eq!(resumed.request.operation, assignment.request.operation);
                check_eq!(resumed.policy, policy);
                check_eq!(resumed.deadline_ms, assignment.deadline_ms);
                check_eq!(resumed.dispatches, 1);
                let second = require_owned_grant(
                    catalog.begin_owned_work(&resumed)?,
                    "missing second grant",
                )?;
                check_eq!(second.ordinal, 2);
                check_eq!(second.assignment.deadline_ms, assignment.deadline_ms);
            }
        }
        Ok(())
    }

    #[test]
    fn equal_catalog_clock_sample_skips_update_and_advance_survives_reopen() -> TestResult {
        let root = tempfile::tempdir()?;
        let clock = Arc::new(ManualClock::new(1_000));
        let (catalog, _genesis) = setup_with_clock(root.path(), clock.clone())?;
        {
            let mut guard = catalog.lock()?;
            let tx = begin(&mut guard)?;
            let _seeded = catalog_now_ms(&tx, 1_000)?;
            tx.commit()?;
            drop(guard);
        }
        {
            let mut guard = catalog.lock()?;
            let tx = begin(&mut guard)?;
            let before = tx.total_changes();
            check_eq!(catalog_now_ms(&tx, 1_000)?, 1_000);
            check_eq!(tx.total_changes(), before);
            tx.commit()?;
            check_eq!(guard.total_changes(), before);
            drop(guard);
        }
        {
            let mut guard = catalog.lock()?;
            let tx = begin(&mut guard)?;
            check_eq!(catalog_now_ms(&tx, 1_001)?, 1_001);
            tx.commit()?;
            drop(guard);
        }
        clock.set(1_001);
        drop(catalog);
        let reopened =
            SqliteCatalog::open_with_clock(root.path(), Duration::from_secs(1), clock.clone())?;
        let guard = reopened.lock()?;
        let floor: i64 = guard.query_row(
            "SELECT last_ms FROM native_work_clock_v1 WHERE singleton=1",
            [],
            |row| row.get(0),
        )?;
        check_eq!(floor, 1_001);
        clock.set(1_000);
        check!(matches!(
            catalog_now_ms(&guard, clock.now_unix_ms()),
            Err(IndexError::Storage(reason)) if reason == "catalog clock regressed"
        ));
        drop(guard);
        Ok(())
    }

    #[test]
    fn equal_catalog_clock_sample_still_rejects_corrupt_or_missing_floor() -> TestResult {
        for (mutation, corrupt_digest) in [
            (
                "UPDATE native_work_clock_v1 SET row_digest=X'00' WHERE singleton=1",
                true,
            ),
            ("DELETE FROM native_work_clock_v1 WHERE singleton=1", false),
        ] {
            let root = tempfile::tempdir()?;
            let clock = Arc::new(ManualClock::new(1_000));
            let (catalog, _genesis) = setup_with_clock(root.path(), clock)?;
            let guard = catalog.lock()?;
            let _seeded = catalog_now_ms(&guard, 1_000)?;
            guard.execute_batch(mutation)?;
            let result = catalog_now_ms(&guard, 1_000);
            drop(guard);
            if corrupt_digest {
                check!(matches!(
                    result,
                    Err(IndexError::Storage(reason)) if reason == "native work clock digest mismatch"
                ));
            } else {
                check!(matches!(result, Err(IndexError::Storage(_))));
            }
        }
        Ok(())
    }

    fn owned_finish_fixture(
        catalog: &SqliteCatalog,
        genesis: &CatalogView,
        complete: bool,
    ) -> Result<(WorkIntent, OwnedWorkGrant, Option<ViewRef>), Box<dyn Error>> {
        let intent = owner_work(179);
        let queued = publish_work_next(catalog, genesis, 1, &intent, 12_179, 179)?;
        let assignment = catalog
            .acquire_owned_work(&intent, retirement_owned_policy(), &admission_budget())?
            .ok_or("missing owned assignment")?;
        let grant = require_owned_grant(catalog.begin_owned_work(&assignment)?, "missing grant")?;
        if !complete {
            return Ok((intent, grant, None));
        }
        let request = object(180, vec![]);
        let immutable = ImmutableOperation {
            key: assignment.request.operation,
            request: request.reference,
            source_mode: SourceOperationMode::Unchanged,
        };
        let ingress = stage_inputs(catalog, immutable, std::slice::from_ref(&request))?;
        let attempt = match catalog.admit_common(
            immutable,
            ingress,
            &[],
            Some(&assignment.request),
            &mut admission_budget(),
        )? {
            AdmissionOutcome::New(attempt) => attempt,
            AdmissionOutcome::Existing(status) => {
                return Err(format!("expected fresh owned admission, got {status:?}").into());
            }
        };
        let artifact = object(181, vec![]);
        reserve_confirm(catalog, attempt, std::slice::from_ref(&artifact))?;
        let UpdateKind::Materialize { claim } = assignment.request.kind else {
            return Err("owned request is not materialization".into());
        };
        catalog.stage_work(&claim, owner_result(&intent, artifact.reference)?)?;
        let mut completed = view(2, genesis.profile, None);
        completed.coverage = MapRoot(Some(artifact.reference));
        completed.identity.digest = completed.identity_digest()?;
        check_eq!(
            commit_empty(
                catalog,
                PreparedPublication {
                    attempt,
                    source_mode: SourceOperationMode::Unchanged,
                    expected: queued.identity,
                    next: completed.clone(),
                    work_actions: WorkActionSet::empty(),
                    complete_work: Some(claim),
                    source_events: vec![],
                    projection_events: vec![],
                    expected_source: None,
                },
            )?,
            OperationStatus::Published(completed.identity)
        );
        Ok((intent, grant, Some(completed.identity)))
    }

    #[test]
    fn owned_completed_finish_replays_without_clock_write_and_survives_reopen() -> TestResult {
        let root = tempfile::tempdir()?;
        let clock = Arc::new(ManualClock::new(1_000));
        let (catalog, genesis) = setup_with_clock(root.path(), clock.clone())?;
        let (intent, grant, completed) = owned_finish_fixture(&catalog, &genesis, true)?;
        let completed = completed.ok_or("missing completed view")?;
        let before = {
            let guard = catalog.lock()?;
            guard.total_changes()
        };
        clock.set(1_500);
        catalog.finish_owned_work(&grant, OwnedWorkReport::Reconcile)?;
        clock.set(1_750);
        catalog.finish_owned_work(&grant, OwnedWorkReport::Interrupted)?;
        {
            let guard = catalog.lock()?;
            check_eq!(guard.total_changes(), before);
            let floor: i64 = guard.query_row(
                "SELECT last_ms FROM native_work_clock_v1 WHERE singleton=1",
                [],
                |row| row.get(0),
            )?;
            check_eq!(floor, 1_000);
            let assignment = owned_work::read_owned(&guard, &intent)?.ok_or("lost assignment")?;
            drop(guard);
            check!(!assignment.active);
            check_eq!(assignment.due_ms, u64::try_from(i64::MAX)?);
        }
        drop(catalog);
        let reopened = SqliteCatalog::open_with_clock(root.path(), Duration::from_secs(1), clock)?;
        check_eq!(
            reopened.work_status(&intent, &admission_budget())?,
            WorkStatus::Completed(completed)
        );
        check_eq!(
            inspect_with_test_budget(&reopened, grant.assignment.request.operation)?,
            OperationStatus::Published(completed)
        );
        check_eq!(
            reopened.ready_owned_work(workspace(), 7, 1, &mut admission_budget())?,
            OwnedReadyOutcome::Ready(vec![])
        );
        reopened.finish_owned_work(&grant, OwnedWorkReport::Reconcile)?;
        Ok(())
    }

    #[test]
    fn owned_completed_finish_checks_clock_before_ordinal_and_receipt() -> TestResult {
        for fault in [
            "regressed",
            "digest",
            "missing",
            "out_of_range",
            "owned_digest",
        ] {
            let root = tempfile::tempdir()?;
            let clock = Arc::new(ManualClock::new(1_000));
            let (catalog, genesis) = setup_with_clock(root.path(), clock.clone())?;
            let (intent, grant, _completed) = owned_finish_fixture(&catalog, &genesis, true)?;
            let mut wrong_ordinal = grant.clone();
            wrong_ordinal.ordinal = wrong_ordinal
                .ordinal
                .checked_add(1)
                .ok_or("ordinal overflow")?;
            match fault {
                "regressed" => clock.set(999),
                "digest" => {
                    let guard = catalog.lock()?;
                    let _changed = guard.execute(
                        "UPDATE native_work_clock_v1 SET row_digest=X'00' WHERE singleton=1",
                        [],
                    )?;
                    drop(guard);
                }
                "missing" => {
                    let guard = catalog.lock()?;
                    let _changed =
                        guard.execute("DELETE FROM native_work_clock_v1 WHERE singleton=1", [])?;
                    drop(guard);
                }
                "out_of_range" => clock.set(
                    u64::try_from(i64::MAX)?
                        .checked_add(1)
                        .ok_or("clock sample overflow")?,
                ),
                "owned_digest" => {
                    let guard = catalog.lock()?;
                    let _changed = guard.execute(
                        "UPDATE native_owned_work_v1 SET row_digest=X'00' WHERE work=?1",
                        params![work_key(&intent)?],
                    )?;
                    drop(guard);
                }
                _ => return Err("unknown clock fault".into()),
            }
            let result = catalog.finish_owned_work(&wrong_ordinal, OwnedWorkReport::Reconcile);
            match fault {
                "regressed" => check!(matches!(
                    result,
                    Err(IndexError::Storage(reason)) if reason == "catalog clock regressed"
                )),
                "digest" => check!(matches!(
                    result,
                    Err(IndexError::Storage(reason)) if reason == "native work clock digest mismatch"
                )),
                "missing" | "owned_digest" => {
                    check!(matches!(result, Err(IndexError::Storage(_))));
                }
                "out_of_range" => check!(matches!(
                    result,
                    Err(IndexError::InvalidInput(reason))
                        if reason == "native counter exceeds SQLite integer domain"
                )),
                _ => return Err("unknown clock fault".into()),
            }
        }
        Ok(())
    }

    #[test]
    fn owned_live_finish_persists_clock_and_terminal_finish_rejects_wrong_receipt() -> TestResult {
        let root = tempfile::tempdir()?;
        let clock = Arc::new(ManualClock::new(1_000));
        let (catalog, genesis) = setup_with_clock(root.path(), clock.clone())?;
        let (intent, grant, _completed) = owned_finish_fixture(&catalog, &genesis, false)?;
        clock.set(1_500);
        catalog.finish_owned_work(&grant, OwnedWorkReport::Interrupted)?;
        let floor: i64 = catalog.lock()?.query_row(
            "SELECT last_ms FROM native_work_clock_v1 WHERE singleton=1",
            [],
            |row| row.get(0),
        )?;
        check_eq!(floor, 1_500);
        let assignment = {
            let guard = catalog.lock()?;
            let assignment = owned_work::read_owned(&guard, &intent)?.ok_or("lost assignment")?;
            drop(guard);
            assignment
        };
        check!(!assignment.active);
        clock.set(1_499);
        check!(matches!(
            catalog.finish_owned_work(&grant, OwnedWorkReport::Interrupted),
            Err(IndexError::Storage(reason)) if reason == "catalog clock regressed"
        ));

        let other_root = tempfile::tempdir()?;
        let other_clock = Arc::new(ManualClock::new(1_000));
        let (other, other_genesis) = setup_with_clock(other_root.path(), other_clock)?;
        let (_intent, grant, _completed) = owned_finish_fixture(&other, &other_genesis, true)?;
        let mut wrong_ordinal = grant.clone();
        wrong_ordinal.ordinal = wrong_ordinal
            .ordinal
            .checked_add(1)
            .ok_or("ordinal overflow")?;
        check_eq!(
            other.finish_owned_work(&wrong_ordinal, OwnedWorkReport::Reconcile),
            Err(IndexError::WorkFenceLost)
        );
        {
            let mut guard = other.lock()?;
            let tx = begin(&mut guard)?;
            let mut receipt = read_operation(&tx, grant.assignment.request.operation)?
                .ok_or("missing published receipt")?;
            receipt.status = OperationStatus::Refused(Refusal::Conflict);
            write_operation(&tx, &receipt, false)?;
            tx.commit()?;
            drop(guard);
        }
        check_eq!(
            other.finish_owned_work(&grant, OwnedWorkReport::Reconcile),
            Err(IndexError::WorkFenceLost)
        );
        Ok(())
    }

    #[test]
    fn owned_ready_discovery_is_read_only_and_checks_durable_clock_floor() -> TestResult {
        let root = tempfile::tempdir()?;
        let clock = Arc::new(ManualClock::new(1_000));
        let (catalog, _genesis) = setup_with_clock(root.path(), clock.clone())?;
        let changes = {
            let guard = catalog.lock()?;
            let observed = catalog_now_ms(&guard, 1_000)?;
            check_eq!(observed, 1_000);
            guard.total_changes()
        };
        clock.set(2_000);
        for _poll in 0..3 {
            check_eq!(
                catalog.ready_owned_work(workspace(), 7, 1, &mut admission_budget())?,
                OwnedReadyOutcome::Ready(vec![])
            );
            check!(
                catalog
                    .ready_work(workspace(), 7, 1, &mut admission_budget())?
                    .is_empty()
            );
        }
        {
            let guard = catalog.lock()?;
            let observed_changes = guard.total_changes();
            let floor: i64 = guard.query_row(
                "SELECT last_ms FROM native_work_clock_v1 WHERE singleton=1",
                [],
                |row| row.get(0),
            )?;
            drop(guard);
            check_eq!(observed_changes, changes);
            check_eq!(floor, 1_000);
        }
        clock.set(999);
        check!(
            matches!(catalog.ready_owned_work(workspace(), 7, 1, &mut admission_budget()), Err(IndexError::Storage(reason)) if reason == "catalog clock regressed")
        );
        check!(
            matches!(catalog.ready_work(workspace(), 7, 1, &mut admission_budget()), Err(IndexError::Storage(reason)) if reason == "catalog clock regressed")
        );
        let observed_changes = catalog.lock()?.total_changes();
        check_eq!(observed_changes, changes);
        Ok(())
    }

    #[test]
    fn owned_ready_key_lookup_keeps_canonical_work_row_checks() -> TestResult {
        for case in [
            "valid",
            "digest",
            "digest_shadow",
            "wrong_key",
            "other_row",
            "other_corrupt",
            "shadow",
            "claim",
        ] {
            let root = tempfile::tempdir()?;
            let clock = Arc::new(ManualClock::new(1_000));
            let (catalog, genesis) = setup_with_clock(root.path(), clock.clone())?;
            let intent = owner_work(172);
            let published = publish_work_next(&catalog, &genesis, 1, &intent, 12_060, 172)?;
            if case == "other_row" || case == "other_corrupt" {
                clock.set(1_001);
                let other = owner_work(173);
                let _other_published =
                    publish_work_next(&catalog, &published, 2, &other, 12_061, 173)?;
                if case == "other_corrupt" {
                    let guard = catalog.lock()?;
                    check_eq!(
                        guard.execute(
                            "UPDATE native_work_v1 SET row_digest=zeroblob(32) WHERE work=?1",
                            params![work_key(&other)?],
                        )?,
                        1
                    );
                    drop(guard);
                }
            }
            if case == "claim" {
                let policy = OwnedWorkPolicy {
                    dispatch_limits: WorkLimits {
                        reads: 100,
                        writes: 100,
                        bytes: 4096,
                        mutations: 100,
                    },
                    max_dispatches: 2,
                    total_timeout_ms: 10_000,
                    lease_ms: 2_000,
                    retry_backoff_ms: 100,
                };
                let _assigned = catalog
                    .acquire_owned_work(&intent, policy, &admission_budget())?
                    .ok_or("missing claimed assignment")?;
            }
            let key = work_key(&intent)?;
            let row = {
                let guard = catalog.lock()?;
                read_work(&guard, &intent)?.ok_or("missing work row")?
            };
            {
                let guard = catalog.lock()?;
                let changed = match case {
                    "valid" => 1,
                    "digest" => guard.execute(
                        "UPDATE native_work_v1 SET row_digest=zeroblob(32) WHERE work=?1",
                        params![&key],
                    )?,
                    "digest_shadow" => guard.execute(
                        "UPDATE native_work_v1 SET row_digest=zeroblob(32),ready_since_ms=ready_since_ms+1 WHERE work=?1",
                        params![&key],
                    )?,
                    "wrong_key" | "other_row" | "other_corrupt" | "claim" => {
                        let mut forged = row.clone();
                        if case == "wrong_key"
                            || case == "other_row"
                            || case == "other_corrupt"
                        {
                            forged.intent = owner_work(173);
                        } else {
                            let WorkStatus::Claimed(mut claim) = forged.status else {
                                return Err("expected claimed work row".into());
                            };
                            claim.fence = 0;
                            forged.status = WorkStatus::Claimed(claim);
                        }
                        let payload = encode(&forged)?;
                        let row_digest = digest(b"qidx.native.work.v2", &payload);
                        guard.execute(
                            "UPDATE native_work_v1 SET payload=?2,row_digest=?3 WHERE work=?1",
                            params![&key, payload, row_digest.to_vec()],
                        )?
                    }
                    "shadow" => guard.execute(
                        "UPDATE native_work_v1 SET ready_since_ms=ready_since_ms+1 WHERE work=?1",
                        params![&key],
                    )?,
                    _ => return Err("unknown owned ready case".into()),
                };
                check_eq!(changed, 1);
                drop(guard);
            }
            if case == "digest_shadow" {
                let guard = catalog.lock()?;
                check!(matches!(
                    read_work(&guard, &intent),
                    Err(IndexError::Storage(reason)) if reason == "native catalog row digest mismatch"
                ));
                drop(guard);
            }
            let ready = catalog.ready_owned_work(workspace(), 7, 1, &mut admission_budget());
            match case {
                "valid" => check_eq!(ready?, OwnedReadyOutcome::Ready(vec![intent])),
                "digest" | "digest_shadow" | "other_corrupt" => check!(matches!(
                    ready,
                    Err(IndexError::Storage(reason)) if reason == "native catalog row digest mismatch"
                )),
                "wrong_key" => check!(matches!(
                    ready,
                    Err(IndexError::Storage(reason)) if reason == "owned ready work disappeared"
                )),
                "other_row" => check!(matches!(
                    ready,
                    Err(IndexError::Storage(reason)) if reason == "owned ready scope mismatch"
                )),
                "shadow" => check!(matches!(
                    ready,
                    Err(IndexError::Storage(reason)) if reason == "native work row identity mismatch"
                )),
                "claim" => check!(matches!(
                    ready,
                    Err(IndexError::Storage(reason)) if reason == "native work claim identity mismatch"
                )),
                _ => return Err("unknown owned ready case".into()),
            }
        }
        Ok(())
    }

    #[test]
    fn owned_conflict_rotation_is_single_and_preserves_new_assignment() -> TestResult {
        use quanta_index_core::index::{OwnedWorkPolicy, OwnedWorkReport};
        // Both acquisition after expiry and finish reconciliation must rotate
        // once. Inject a terminal receipt through the real Catalog row writer;
        // Kernel integration tests separately exercise canonical admission.
        for (finish, reason) in [
            (false, Refusal::Conflict),
            (true, Refusal::Conflict),
            (false, Refusal::FenceLost),
            (true, Refusal::FenceLost),
        ] {
            let root = tempfile::tempdir()?;
            let clock = Arc::new(ManualClock::new(1_000));
            let (catalog, genesis) = setup_with_clock(root.path(), clock.clone())?;
            let first = operation(244, 245);
            let attempt = admit_new(&catalog, first)?;
            let intent = owner_work(93);
            let next = view(1, genesis.profile, None);
            let publication = commit_actions(
                &catalog,
                PreparedPublication {
                    attempt,
                    expected: genesis.identity,
                    next,
                    work_actions: WorkActionSet::empty(),
                    complete_work: None,
                    source_events: vec![],
                    projection_events: vec![],
                    expected_source: None,
                    source_mode: SourceOperationMode::Unchanged,
                },
                &[WorkAction::Create(intent)],
            )?;
            check!(matches!(publication, OperationStatus::Published(_)));
            let policy = OwnedWorkPolicy {
                dispatch_limits: WorkLimits {
                    reads: 100,
                    writes: 100,
                    bytes: 4096,
                    mutations: 100,
                },
                max_dispatches: 3,
                total_timeout_ms: 100,
                lease_ms: 20,
                retry_backoff_ms: 1,
            };
            let assignment = catalog
                .acquire_owned_work(&intent, policy, &admission_budget())?
                .ok_or("missing assignment")?;
            let grant =
                require_owned_grant(catalog.begin_owned_work(&assignment)?, "missing grant")?;
            {
                let mut guard = catalog.lock()?;
                let tx = begin(&mut guard)?;
                write_operation(
                    &tx,
                    &OperationRow {
                        immutable: ImmutableOperation {
                            key: grant.assignment.request.operation,
                            request: ObjectRef(digest_id(246)),
                            source_mode: SourceOperationMode::Unchanged,
                        },
                        events: vec![],
                        fence: next_fence(&tx)?,
                        status: OperationStatus::Refused(reason),
                    },
                    true,
                )?;
                tx.commit()?;
                drop(guard);
            }
            clock.set(1_021);
            let rotated = if finish {
                catalog.finish_owned_work(&grant, OwnedWorkReport::Interrupted)?;
                catalog
                    .acquire_owned_work(&intent, policy, &admission_budget())?
                    .ok_or("missing finished rotation")?
            } else {
                catalog
                    .acquire_owned_work(&intent, policy, &admission_budget())?
                    .ok_or("missing expiry rotation")?
            };
            check_ne!(rotated.request.operation, assignment.request.operation);
            check_eq!(rotated.dispatches, 1);
            check_eq!(rotated.policy, policy);
            check_eq!(rotated.deadline_ms, assignment.deadline_ms);
            let guard = catalog.lock()?;
            let retained: i64 =
                guard.query_row("SELECT COUNT(*) FROM native_owned_request_v1", [], |row| {
                    row.get(0)
                })?;
            check_eq!(retained, 2);
            drop(guard);
            clock.set(1_022);
            let second = require_owned_grant(
                catalog.begin_owned_work(&rotated)?,
                "missing rotated dispatch",
            )?;
            check_eq!(second.ordinal, 2);
            let _validated_status = catalog.validate_owned_work(&second.assignment.request)?;
            check_eq!(
                catalog.finish_owned_work(&grant, OwnedWorkReport::Interrupted),
                Err(IndexError::WorkFenceLost)
            );
            check_eq!(
                inspect_with_test_budget(&catalog, assignment.request.operation)?,
                OperationStatus::Refused(reason)
            );
        }
        Ok(())
    }

    #[test]
    fn owned_refusal_rotation_retains_exhaustion_and_permanent_failure() -> TestResult {
        for finish in [false, true] {
            for (reason, now, dispatches, expected) in [
                (Refusal::FenceLost, 1_021, 1, Refusal::ResourceLimited),
                (Refusal::FenceLost, 1_100, 3, Refusal::ResourceLimited),
                (Refusal::InvalidInput, 1_021, 3, Refusal::InvalidInput),
                (
                    Refusal::BackendViolation,
                    1_021,
                    3,
                    Refusal::BackendViolation,
                ),
            ] {
                let root = tempfile::tempdir()?;
                let clock = Arc::new(ManualClock::new(1_000));
                let (catalog, genesis) = setup_with_clock(root.path(), clock.clone())?;
                let intent = owner_work(93);
                let _view = publish_work_next(&catalog, &genesis, 1, &intent, 244, 245)?;
                let policy = OwnedWorkPolicy {
                    max_dispatches: dispatches,
                    total_timeout_ms: 100,
                    lease_ms: 20,
                    ..retirement_owned_policy()
                };
                let assignment = catalog
                    .acquire_owned_work(&intent, policy, &admission_budget())?
                    .ok_or("missing assignment")?;
                let grant =
                    require_owned_grant(catalog.begin_owned_work(&assignment)?, "missing grant")?;
                insert_refused_owned_receipt(&catalog, assignment.request.operation, reason)?;
                clock.set(now);
                if finish {
                    catalog.finish_owned_work(&grant, OwnedWorkReport::Interrupted)?;
                }
                check_eq!(
                    catalog.acquire_owned_work(&intent, policy, &admission_budget())?,
                    None
                );
                check_eq!(
                    catalog.work_status(&intent, &admission_budget())?,
                    WorkStatus::Blocked(expected)
                );
                check_eq!(
                    inspect_with_test_budget(&catalog, assignment.request.operation)?,
                    OperationStatus::Refused(reason)
                );
            }
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
        let session = catalog.open_read_session()?;
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
            source_mode: SourceOperationMode::Unchanged,
        };
        stage_action_set(&mut publication, &[WorkAction::Create(intent)])?;
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
        let mut declared_empty = publication.clone();
        declared_empty.work_actions = WorkActionSet::empty();
        let mut unexpected = VecActions(VecDeque::from([WorkAction::Create(intent)]));
        check!(matches!(
            catalog.commit_transition(declared_empty.clone(), &mut unexpected, &mut new_budget()),
            Err(IndexError::CorruptObject(_))
        ));
        declared_empty.work_actions.digest = Digest([0; 32]);
        let mut empty = VecActions(VecDeque::new());
        check!(matches!(
            catalog.commit_transition(declared_empty, &mut empty, &mut new_budget()),
            Err(IndexError::CorruptObject(_))
        ));
        let mut empty = VecActions(VecDeque::new());
        check!(matches!(
            catalog.commit_transition(publication.clone(), &mut empty, &mut new_budget()),
            Err(IndexError::CorruptObject(_))
        ));
        let mut interrupted = FailAfterOne(Some(WorkAction::Create(intent)));
        check!(matches!(
            catalog.commit_transition(publication.clone(), &mut interrupted, &mut new_budget()),
            Err(IndexError::BackendViolation(_))
        ));
        let first = WorkAction::Create(intent);
        let second = WorkAction::Create(owner_work(7));
        let ordered = if first.key()? < second.key()? {
            [first, second]
        } else {
            [second, first]
        };
        let mut extra = VecActions(VecDeque::from(ordered));
        check!(matches!(
            catalog.commit_transition(publication.clone(), &mut extra, &mut new_budget()),
            Err(IndexError::CorruptObject(_))
        ));
        let mut wrong_digest = publication.clone();
        wrong_digest.work_actions.digest = Digest([0; 32]);
        let mut complete = VecActions(VecDeque::from([WorkAction::Create(intent)]));
        check!(matches!(
            catalog.commit_transition(wrong_digest, &mut complete, &mut new_budget()),
            Err(IndexError::CorruptObject(_))
        ));
        let mut missing_root = owner_work(8);
        let WorkScope::Owner { expected, .. } = &mut missing_root.scope else {
            return Err("owner work fixture changed".into());
        };
        let work_root = ObjectRef(digest_id(221));
        expected.source = Some(work_root);
        let mut missing_publication = publication.clone();
        stage_action_set(
            &mut missing_publication,
            &[WorkAction::Create(missing_root)],
        )?;
        let mut missing_cursor = VecActions(VecDeque::from([WorkAction::Create(missing_root)]));
        check!(matches!(
            catalog.commit_transition(
                missing_publication.clone(),
                &mut missing_cursor,
                &mut new_budget(),
            ),
            Err(IndexError::CorruptObject(_))
        ));
        check_eq!(
            inspect_with_test_budget(&catalog, operation.key)?,
            OperationStatus::Pending(attempt)
        );
        check_eq!(
            catalog.work_status(&intent, &admission_budget())?,
            WorkStatus::Unknown
        );
        check_eq!(
            catalog.work_status(&owner_work(7), &admission_budget())?,
            WorkStatus::Unknown
        );
        let ticket = acquire_ticket_with_test_budget(
            &catalog,
            session.as_ref(),
            ViewSelection::Latest {
                workspace: workspace(),
                epoch: 7,
            },
            Duration::from_secs(30),
        )?;
        check_eq!(ticket.view.identity, genesis.identity);
        catalog.release_ticket(session.as_ref(), ticket.lease)?;
        reserve_confirm(
            &catalog,
            attempt,
            &[RegisteredObject::unscoped(work_root, 1, vec![])],
        )?;
        let mut valid = VecActions(VecDeque::from([WorkAction::Create(missing_root)]));
        check_eq!(
            catalog.commit_transition(missing_publication, &mut valid, &mut new_budget())?,
            OperationStatus::Published(next.identity)
        );
        check_eq!(
            catalog.work_status(&missing_root, &admission_budget())?,
            WorkStatus::Pending
        );
        let work_children: i64 = catalog.lock()?.query_row(
            "SELECT COUNT(*) FROM native_work_object_v1 WHERE work=?1 AND object=?2",
            params![work_key(&missing_root)?, object_key(work_root)],
            |row| row.get(0),
        )?;
        check_eq!(work_children, 1);
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
        check_eq!(
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
                    source_mode: SourceOperationMode::Unchanged,
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
        check_eq!(
            catalog.ready_work(workspace(), 7, 1, &mut budget)?,
            vec![intent]
        );
        check_eq!(
            catalog.ready_work(workspace(), 7, 257, &mut budget),
            Err(IndexError::ResourceLimited)
        );
        let claim = catalog.claim_work(&intent, Duration::from_secs(30), &admission_budget())?;
        check_eq!(
            catalog.work_status(&intent, &admission_budget())?,
            WorkStatus::Claimed(claim)
        );
        let artifact = object(202, vec![]);
        let second = operation(201, 203);
        let second_attempt = admit_new(&catalog, second)?;
        reserve_confirm(&catalog, second_attempt, std::slice::from_ref(&artifact))?;
        let shadows_before = {
            let guard = catalog.lock()?;
            guard.query_row(
                "SELECT operation,workspace,epoch,sequence,status,owner_mode,ready_since_ms,lease_until_ms
                 FROM native_work_v1 WHERE work=?1",
                params![work_key(&intent)?],
                |row| {
                    (0..8)
                        .map(|column| row.get::<_, rusqlite::types::Value>(column))
                        .collect::<Result<Vec<_>, _>>()
                },
            )?
        };
        let mut wrong = owner_result(&intent, artifact.reference)?;
        wrong.input.derivation_policy = digest_id(99);
        check!(matches!(
            catalog.stage_work(&claim, wrong),
            Err(IndexError::BackendViolation(_))
        ));
        let result = owner_result(&intent, artifact.reference)?;
        catalog.stage_work(&claim, result.clone())?;
        check_eq!(catalog.prepared_work(&claim)?, Some(result.clone()));
        check_eq!(
            catalog.work_status(&intent, &admission_budget())?,
            WorkStatus::Claimed(claim)
        );
        let shadows_after = {
            let guard = catalog.lock()?;
            guard.query_row(
                "SELECT operation,workspace,epoch,sequence,status,owner_mode,ready_since_ms,lease_until_ms
                 FROM native_work_v1 WHERE work=?1",
                params![work_key(&intent)?],
                |row| {
                    (0..8)
                        .map(|column| row.get::<_, rusqlite::types::Value>(column))
                        .collect::<Result<Vec<_>, _>>()
                },
            )?
        };
        check_eq!(shadows_after, shadows_before);
        let indexed_claims: i64 = catalog.lock()?.query_row(
            "SELECT COUNT(*) FROM native_work_v1 INDEXED BY native_owned_claim_idx_v1
             WHERE workspace=?1 AND epoch=?2 AND owner_mode=?3 AND status=?4
               AND lease_until_ms=?5 AND work=?6",
            params![
                workspace_key(workspace())?,
                7_i64,
                1_i64,
                1_i64,
                int(claim.lease_until_ms)?,
                work_key(&intent)?
            ],
            |row| row.get(0),
        )?;
        check_eq!(indexed_claims, 1);
        catalog.stage_work(&claim, result.clone())?;
        check_eq!(catalog.prepared_work(&claim)?, Some(result.clone()));
        let conflicting = owner_result(&intent, genesis.profile)?;
        check_eq!(
            catalog.stage_work(&claim, conflicting),
            Err(IndexError::DigestConflict)
        );
        check_eq!(catalog.prepared_work(&claim)?, Some(result.clone()));
        drop(catalog);

        let catalog = open(root.path())?;
        check_eq!(catalog.prepared_work(&claim)?, Some(result));
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
            source_mode: SourceOperationMode::Unchanged,
        };
        check_eq!(
            commit_empty(&catalog, publication.clone()),
            Err(IndexError::Conflict)
        );
        check_eq!(
            catalog.work_status(&intent, &admission_budget())?,
            WorkStatus::Claimed(claim)
        );
        check_eq!(
            inspect_with_test_budget(&catalog, second.key)?,
            OperationStatus::Pending(second_attempt)
        );
        let mut current_publication = publication;
        current_publication.expected = queued.identity;
        check_eq!(
            commit_empty(&catalog, current_publication)?,
            OperationStatus::Published(completed_view.identity)
        );
        check_eq!(
            catalog.work_status(&intent, &admission_budget())?,
            WorkStatus::Completed(completed_view.identity)
        );
        check_eq!(
            catalog.claim_work(&intent, Duration::from_secs(1), &admission_budget()),
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
            WorkScope::ProjectionDomain { .. } => {
                return Err("owner fixture unexpectedly has projection work".into());
            }
        };
        let third = operation(202, 204);
        let third_attempt = admit_new(&catalog, third)?;
        let mut changed_view = view(3, genesis.profile, None);
        changed_view.coverage = MapRoot(Some(artifact.reference));
        changed_view.identity.digest = changed_view.identity_digest()?;
        check_eq!(
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
                    source_mode: SourceOperationMode::Unchanged,
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
        check_eq!(
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
                    source_mode: SourceOperationMode::Unchanged,
                },
                &[
                    WorkAction::Retire(changed_stamp),
                    WorkAction::Create(intent)
                ]
            )?,
            OperationStatus::Published(restored_view.identity)
        );
        check_eq!(
            catalog.work_status(&intent, &admission_budget())?,
            WorkStatus::Pending
        );
        check_eq!(
            catalog.work_status(&changed_stamp, &admission_budget())?,
            WorkStatus::Superseded
        );
        let rearmed = catalog.claim_work(&intent, Duration::from_secs(5), &admission_budget())?;
        check_ne!(rearmed.fence, claim.fence);
        check_eq!(
            catalog.prepared_work(&rearmed)?,
            Some(owner_result(&intent, artifact.reference)?)
        );
        check_eq!(
            catalog.prepared_work(&claim),
            Err(IndexError::WorkFenceLost)
        );
        check_eq!(
            inspect_with_test_budget(&catalog, second.key)?,
            OperationStatus::Published(completed_view.identity)
        );
        Ok(())
    }

    #[test]
    fn prepared_work_reads_preserve_clock_floor_and_replay_checks() -> TestResult {
        let root = tempfile::tempdir()?;
        let clock = Arc::new(ManualClock::new(1_000));
        let (catalog, genesis) = setup_with_clock(root.path(), clock.clone())?;
        let intent = owner_work(96);
        let _queued = publish_work_next(&catalog, &genesis, 1, &intent, 401, 202)?;
        let claim = catalog.claim_work(&intent, Duration::from_secs(30), &admission_budget())?;
        let before = {
            let guard = catalog.lock()?;
            let row: (i64, Vec<u8>) = guard.query_row(
                "SELECT last_ms,row_digest FROM native_work_clock_v1 WHERE singleton=1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            (guard.total_changes(), row)
        };
        clock.set(1_001);
        check_eq!(catalog.prepared_work(&claim)?, None);
        let after = {
            let guard = catalog.lock()?;
            let row: (i64, Vec<u8>) = guard.query_row(
                "SELECT last_ms,row_digest FROM native_work_clock_v1 WHERE singleton=1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            (guard.total_changes(), row)
        };
        check_eq!(after, before);

        let artifact = object(203, vec![]);
        let attempt = admit_new(&catalog, operation(402, 204))?;
        reserve_confirm(&catalog, attempt, std::slice::from_ref(&artifact))?;
        let binding = owner_result(&intent, artifact.reference)?;
        catalog.stage_work(&claim, binding.clone())?;
        let staged_floor: i64 = catalog.lock()?.query_row(
            "SELECT last_ms FROM native_work_clock_v1 WHERE singleton=1",
            [],
            |row| row.get(0),
        )?;
        check_eq!(staged_floor, 1_001);
        drop(catalog);

        let reopened =
            SqliteCatalog::open_with_clock(root.path(), Duration::from_secs(1), clock.clone())?;
        clock.set(1_002);
        let before_replay = {
            let guard = reopened.lock()?;
            let row: (i64, Vec<u8>) = guard.query_row(
                "SELECT last_ms,row_digest FROM native_work_clock_v1 WHERE singleton=1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            (guard.total_changes(), row)
        };
        check_eq!(reopened.prepared_work(&claim)?, Some(binding.clone()));
        let after_replay = {
            let guard = reopened.lock()?;
            let row: (i64, Vec<u8>) = guard.query_row(
                "SELECT last_ms,row_digest FROM native_work_clock_v1 WHERE singleton=1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            (guard.total_changes(), row)
        };
        check_eq!(after_replay, before_replay);

        // An advisory observation is not a new durable clock floor. Only the
        // already persisted stage is authoritative after a later clock sample.
        clock.set(1_001);
        check_eq!(reopened.prepared_work(&claim)?, Some(binding.clone()));

        clock.set(1_000);
        check!(matches!(
            reopened.prepared_work(&claim),
            Err(IndexError::Storage(reason)) if reason == "catalog clock regressed"
        ));
        clock.set(claim.lease_until_ms);
        check_eq!(
            reopened.prepared_work(&claim),
            Err(IndexError::WorkFenceLost)
        );
        check_eq!(
            reopened.stage_work(&claim, binding),
            Err(IndexError::WorkFenceLost)
        );
        clock.set(1_002);
        let changed = reopened.lock()?.execute(
            "UPDATE native_work_clock_v1 SET row_digest=X'00' WHERE singleton=1",
            [],
        )?;
        check_eq!(changed, 1);
        check!(matches!(
            reopened.prepared_work(&claim),
            Err(IndexError::Storage(reason)) if reason == "native work clock digest mismatch"
        ));
        Ok(())
    }

    #[test]
    fn stage_work_rejects_corrupt_shadow_without_changing_prepared_payload() -> TestResult {
        let root = tempfile::tempdir()?;
        let (catalog, genesis) = setup(root.path())?;
        let intent = owner_work(97);
        let _queued = publish_work_next(&catalog, &genesis, 1, &intent, 403, 205)?;
        let claim = catalog.claim_work(&intent, Duration::from_secs(30), &admission_budget())?;
        let artifact = object(206, vec![]);
        let attempt = admit_new(&catalog, operation(404, 207))?;
        reserve_confirm(&catalog, attempt, std::slice::from_ref(&artifact))?;
        let key = work_key(&intent)?;
        let before: (Vec<u8>, Vec<u8>) = catalog.lock()?.query_row(
            "SELECT payload,row_digest FROM native_work_v1 WHERE work=?1",
            params![&key],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        let changed = catalog.lock()?.execute(
            "UPDATE native_work_v1 SET ready_since_ms=ready_since_ms+1 WHERE work=?1",
            params![&key],
        )?;
        check_eq!(changed, 1);
        check!(matches!(
            catalog.stage_work(&claim, owner_result(&intent, artifact.reference)?),
            Err(IndexError::Storage(reason)) if reason == "native work row identity mismatch"
        ));
        let after: (Vec<u8>, Vec<u8>) = catalog.lock()?.query_row(
            "SELECT payload,row_digest FROM native_work_v1 WHERE work=?1",
            params![&key],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        check_eq!(after, before);
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
                source_mode: SourceOperationMode::Unchanged,
            },
            &[WorkAction::Create(intent)],
        )?;
        check_eq!(published, OperationStatus::Published(queued.identity));
        let old = catalog.claim_work(&intent, Duration::from_millis(1), &admission_budget())?;
        std::thread::sleep(Duration::from_millis(10));
        drop(catalog);
        let catalog = open(root.path())?;
        let forged = WorkClaim {
            lease_until_ms: u64::MAX,
            ..old
        };
        check_eq!(
            catalog.prepared_work(&forged),
            Err(IndexError::WorkFenceLost)
        );
        let new = catalog.claim_work(&intent, Duration::from_millis(200), &admission_budget())?;
        check_ne!(old.fence, new.fence);
        check_eq!(catalog.prepared_work(&old), Err(IndexError::WorkFenceLost));
        check_eq!(
            catalog.work_status(&intent, &admission_budget())?,
            WorkStatus::Claimed(new)
        );
        let renewed = catalog.renew_work(&new, Duration::from_secs(5), &admission_budget())?;
        check_eq!(renewed.fence, new.fence);
        check!(renewed.lease_until_ms > new.lease_until_ms);
        std::thread::sleep(Duration::from_millis(250));
        check_eq!(catalog.prepared_work(&new)?, None);
        let again = catalog.renew_work(&new, Duration::from_secs(1), &admission_budget())?;
        check_eq!(again.fence, new.fence);
        check_eq!(again.lease_until_ms, renewed.lease_until_ms);
        check_eq!(
            catalog.work_status(&intent, &admission_budget())?,
            WorkStatus::Claimed(renewed)
        );
        Ok(())
    }

    #[test]
    fn restart_replays_original_receipt_and_preserves_exact_old_view() -> TestResult {
        let root = tempfile::tempdir()?;
        let (catalog, genesis) = setup(root.path())?;
        let session = catalog.open_read_session()?;
        let operation = operation(55, 60);
        let attempt = admit_new(&catalog, operation)?;
        let leaf = object(50, vec![]);
        let map = object(51, vec![leaf.reference]);
        reserve_confirm(&catalog, attempt, &[leaf, map.clone()])?;
        let next = view(1, genesis.profile, Some(map.reference));
        check_eq!(
            publish(&catalog, attempt, &genesis, next.clone())?,
            OperationStatus::Published(next.identity)
        );
        drop(catalog);

        let catalog = open(root.path())?;
        check_eq!(
            inspect_with_test_budget(&catalog, operation.key)?,
            OperationStatus::Published(next.identity)
        );
        check_eq!(catalog.operation(operation.key)?, operation);
        check_eq!(
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
        check_eq!(
            catalog.admit(
                conflicting,
                replay_ingress(conflicting),
                &[],
                &mut admission_budget()
            ),
            Err(IndexError::DigestConflict)
        );
        let old = acquire_ticket_with_test_budget(
            &catalog,
            session.as_ref(),
            ViewSelection::Exact(genesis.identity),
            Duration::from_secs(30),
        )?;
        check_eq!(old.view, genesis);
        let new = acquire_ticket_with_test_budget(
            &catalog,
            session.as_ref(),
            ViewSelection::Latest {
                workspace: workspace(),
                epoch: 7,
            },
            Duration::from_secs(30),
        )?;
        check_eq!(new.view, next);
        catalog.release_ticket(session.as_ref(), old.lease)?;
        catalog.release_ticket(session.as_ref(), new.lease)?;
        Ok(())
    }

    #[test]
    fn terminal_operation_replays_digest_and_receipt_without_request_object() -> TestResult {
        let root = tempfile::tempdir()?;
        let (catalog, _) = setup(root.path())?;
        let operation = operation(57, 62);
        let attempt = admit_new(&catalog, operation)?;
        check_eq!(
            catalog.refuse(attempt, Refusal::InvalidInput)?,
            OperationStatus::Refused(Refusal::InvalidInput)
        );
        let direct = Connection::open(catalog.path())?;
        let removed_custody = direct.execute(
            "DELETE FROM native_operation_object_v1 WHERE operation=?1 AND object=?2",
            params![operation_key(operation.key)?, object_key(operation.request)],
        )?;
        check_eq!(removed_custody, 1);
        let removed_object = direct.execute(
            "DELETE FROM native_object_v1 WHERE object=?1",
            params![object_key(operation.request)],
        )?;
        check_eq!(removed_object, 1);
        drop(direct);
        drop(catalog);
        let catalog = open(root.path())?;
        check_eq!(catalog.operation(operation.key)?, operation);
        check_eq!(
            inspect_with_test_budget(&catalog, operation.key)?,
            OperationStatus::Refused(Refusal::InvalidInput)
        );
        check_eq!(
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
        check_eq!(
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
        let session = catalog.open_read_session()?;
        let old_pin = acquire_ticket_with_test_budget(
            &catalog,
            session.as_ref(),
            ViewSelection::Exact(genesis.identity),
            Duration::from_secs(30),
        )?;
        let operation = operation(56, 61);
        let attempt = admit_new(&catalog, operation)?;
        let next = view(1, genesis.profile, Some(genesis.profile));
        check_ne!(next.identity.digest, genesis.identity.digest);
        check_eq!(next.profile, next.members.0.ok_or("member root missing")?);
        check_eq!(
            publish(&catalog, attempt, &genesis, next.clone())?,
            OperationStatus::Published(next.identity)
        );
        let new_pin = acquire_ticket_with_test_budget(
            &catalog,
            session.as_ref(),
            ViewSelection::Exact(next.identity),
            Duration::from_secs(30),
        )?;
        check_eq!(new_pin.view, next);
        check_eq!(old_pin.view, genesis);
        catalog.release_ticket(session.as_ref(), old_pin.lease)?;
        catalog.release_ticket(session.as_ref(), new_pin.lease)?;
        Ok(())
    }

    #[test]
    fn missing_root_conflict_and_stale_fence_leave_pending_unpublished() -> TestResult {
        let root = tempfile::tempdir()?;
        let (catalog, genesis) = setup(root.path())?;
        let first_operation = operation(2, 70);
        let old_attempt = admit_new(&catalog, first_operation)?;
        let missing = ObjectRef(digest_id(99));
        check!(matches!(
            catalog.register_objects(old_attempt, &[object(40, vec![])]),
            Err(IndexError::BackendViolation(_))
        ));
        let invalid = view(1, genesis.profile, Some(missing));
        check!(matches!(
            publish(&catalog, old_attempt, &genesis, invalid),
            Err(IndexError::CorruptObject(_))
        ));
        check_eq!(
            inspect_with_test_budget(&catalog, first_operation.key)?,
            OperationStatus::Pending(old_attempt)
        );
        drop(catalog);

        let catalog = open(root.path())?;
        check_eq!(
            inspect_with_test_budget(&catalog, first_operation.key)?,
            OperationStatus::Pending(old_attempt)
        );
        let new_attempt = catalog.claim_recovery(first_operation.key)?;
        check_ne!(new_attempt, old_attempt);
        check_eq!(
            catalog.register_objects(old_attempt, &[object(41, vec![])]),
            Err(IndexError::FenceLost)
        );
        let leaf = object(42, vec![]);
        reserve_confirm(&catalog, new_attempt, std::slice::from_ref(&leaf))?;
        let next = view(1, genesis.profile, Some(leaf.reference));
        check_eq!(
            publish(&catalog, old_attempt, &genesis, next.clone()),
            Err(IndexError::FenceLost)
        );
        check_eq!(
            publish(&catalog, new_attempt, &genesis, next.clone())?,
            OperationStatus::Published(next.identity)
        );
        let second = operation(3, 71);
        let attempt = admit_new(&catalog, second)?;
        let stale = view(2, genesis.profile, Some(leaf.reference));
        check_eq!(
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
                    source_mode: SourceOperationMode::Unchanged,
                }
            ),
            Err(IndexError::Conflict)
        );
        check_eq!(
            inspect_with_test_budget(&catalog, second.key)?,
            OperationStatus::Pending(attempt)
        );
        Ok(())
    }

    #[test]
    fn source_frontier_has_own_sequence_and_is_atomic_with_view() -> TestResult {
        let root = tempfile::tempdir()?;
        let (catalog, genesis) = setup(root.path())?;
        let first_operation = ImmutableOperation {
            source_mode: SourceOperationMode::Advance,
            ..operation(500, 81)
        };
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
        check_eq!(
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
                    source_mode: SourceOperationMode::Advance,
                }
            )?,
            OperationStatus::Published(next.identity)
        );
        let second = ImmutableOperation {
            source_mode: SourceOperationMode::Advance,
            ..operation(501, 84)
        };
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
        check_eq!(
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
                    source_mode: SourceOperationMode::Advance,
                }
            ),
            Ok(OperationStatus::Published(following.identity))
        );
        check_eq!(
            inspect_with_test_budget(&catalog, second.key)?,
            OperationStatus::Published(following.identity)
        );
        Ok(())
    }

    #[test]
    fn refused_first_source_capture_requires_atomic_producer_resync() -> TestResult {
        let root = tempfile::tempdir()?;
        let (catalog, genesis) = setup(root.path())?;
        let first = ImmutableOperation {
            source_mode: SourceOperationMode::Advance,
            ..operation(1, 87)
        };
        let first_event = SourceEvent {
            producer: first.key.producer,
            stream: first.key.stream,
            stream_epoch: first.key.stream_epoch,
            predecessor: 0,
            sequence: 1,
            payload: digest_id(88),
        };
        let first_attempt = admit_new_events(&catalog, first, &[first_event])?;
        check_eq!(
            catalog.source_epoch(
                genesis.identity,
                first.key.producer,
                &mut admission_budget()
            )?,
            Some(first.key.stream_epoch)
        );
        check_eq!(
            catalog.refuse(first_attempt, Refusal::InvalidInput)?,
            OperationStatus::Refused(Refusal::InvalidInput)
        );
        let forged = ImmutableOperation {
            source_mode: SourceOperationMode::Unchanged,
            ..first
        };
        check_eq!(
            catalog.admit(
                forged,
                replay_ingress(forged),
                &[first_event],
                &mut admission_budget(),
            ),
            Err(IndexError::DigestConflict)
        );

        let next_epoch = first.key.stream_epoch + 1;
        let resync = ImmutableOperation {
            key: OperationKey {
                stream_epoch: next_epoch,
                // Delivery operation ordinal is independent of the new stream's
                // first source event sequence.
                sequence: 77,
                ..first.key
            },
            request: ObjectRef(digest_id(89)),
            source_mode: SourceOperationMode::Resync {
                prior_epoch: first.key.stream_epoch,
            },
        };
        let baseline = SourceEvent {
            stream_epoch: next_epoch,
            payload: digest_id(90),
            ..first_event
        };
        let ingress = stage_inputs(&catalog, resync, &[object(89, vec![])])?;
        let wrong_prior = ImmutableOperation {
            source_mode: SourceOperationMode::Resync {
                prior_epoch: first.key.stream_epoch - 1,
            },
            ..resync
        };
        check_eq!(
            catalog.admit(wrong_prior, ingress, &[baseline], &mut admission_budget()),
            Err(IndexError::Conflict)
        );
        let AdmissionOutcome::New(attempt) =
            catalog.admit(resync, ingress, &[baseline], &mut admission_budget())?
        else {
            return Err("expected new resync attempt".into());
        };
        check_eq!(
            catalog.source_epoch(
                genesis.identity,
                first.key.producer,
                &mut admission_budget()
            )?,
            Some(first.key.stream_epoch)
        );
        let frontier = object(91, vec![]);
        reserve_confirm(&catalog, attempt, std::slice::from_ref(&frontier))?;
        let mut next = view(1, genesis.profile, None);
        next.source.frontiers = MapRoot(Some(frontier.reference));
        next.identity.digest = next.identity_digest()?;
        let publication = PreparedPublication {
            attempt,
            expected: genesis.identity,
            next: next.clone(),
            work_actions: WorkActionSet::empty(),
            complete_work: None,
            source_events: vec![baseline],
            projection_events: vec![],
            expected_source: Some(genesis.source),
            source_mode: resync.source_mode,
        };
        let forged_publication = PreparedPublication {
            source_mode: SourceOperationMode::Advance,
            ..publication.clone()
        };
        check!(matches!(
            commit_empty(&catalog, forged_publication),
            Err(IndexError::InvalidInput(_))
        ));
        check_eq!(
            inspect_with_test_budget(&catalog, resync.key)?,
            OperationStatus::Pending(attempt)
        );
        check_eq!(
            catalog.source_epoch(
                genesis.identity,
                first.key.producer,
                &mut admission_budget()
            )?,
            Some(first.key.stream_epoch)
        );
        check_eq!(
            commit_empty(&catalog, publication)?,
            OperationStatus::Published(next.identity)
        );
        check_eq!(
            catalog.source_epoch(next.identity, first.key.producer, &mut admission_budget())?,
            Some(next_epoch)
        );
        check_eq!(
            catalog.source_epoch(
                genesis.identity,
                first.key.producer,
                &mut admission_budget()
            ),
            Err(IndexError::Conflict)
        );
        drop(catalog);

        let catalog = open(root.path())?;
        check_eq!(
            catalog.source_epoch(next.identity, first.key.producer, &mut admission_budget())?,
            Some(next_epoch)
        );
        let stale = ImmutableOperation {
            key: OperationKey {
                sequence: 2,
                ..first.key
            },
            request: ObjectRef(digest_id(92)),
            source_mode: SourceOperationMode::Advance,
        };
        let stale_event = SourceEvent {
            predecessor: 1,
            sequence: 2,
            payload: digest_id(93),
            ..first_event
        };
        let stale_ingress = stage_inputs(&catalog, stale, &[object(92, vec![])])?;
        check_eq!(
            catalog.admit(
                stale,
                stale_ingress,
                &[stale_event],
                &mut admission_budget()
            ),
            Err(IndexError::Conflict)
        );
        check_eq!(
            inspect_with_test_budget(&catalog, stale.key)?,
            OperationStatus::Unknown
        );
        let corrupted = catalog.lock()?.execute(
            "UPDATE native_source_authority_v1 SET row_digest=zeroblob(32)
             WHERE workspace=?1 AND workspace_epoch=?2 AND producer=?3",
            params![
                workspace_key(first.key.workspace)?,
                int(first.key.epoch)?,
                first.key.producer.0.to_vec()
            ],
        )?;
        check_eq!(corrupted, 1);
        check!(matches!(
            catalog.source_epoch(next.identity, first.key.producer, &mut admission_budget()),
            Err(IndexError::Storage(_))
        ));
        Ok(())
    }

    #[test]
    fn producer_resync_fences_sibling_streams_until_their_baseline() -> TestResult {
        let connection = Connection::open_in_memory()?;
        connection.execute_batch(SCHEMA)?;
        let first = operation(1, 100).key;
        let a = SourceEvent {
            producer: first.producer,
            stream: first.stream,
            stream_epoch: first.stream_epoch,
            predecessor: 0,
            sequence: 1,
            payload: digest_id(101),
        };
        source_authority::admit(
            &connection,
            first,
            SourceOperationMode::Advance,
            &[a],
            &mut admission_budget(),
        )?;
        source_authority::commit(
            &connection,
            first,
            SourceOperationMode::Advance,
            &[a],
            &mut admission_budget(),
        )?;
        install_frontiers(
            &connection,
            first.workspace,
            &[a],
            SourceOperationMode::Advance,
        )?;
        let sibling = OperationKey {
            stream: StreamId(id(10)),
            ..first
        };
        let b = SourceEvent {
            stream: sibling.stream,
            payload: digest_id(102),
            ..a
        };
        source_authority::admit(
            &connection,
            sibling,
            SourceOperationMode::Advance,
            &[b],
            &mut admission_budget(),
        )?;
        install_frontiers(
            &connection,
            first.workspace,
            &[b],
            SourceOperationMode::Advance,
        )?;
        let next_epoch = first.stream_epoch + 1;
        let new_sibling = OperationKey {
            stream_epoch: next_epoch,
            ..sibling
        };
        let b_baseline = SourceEvent {
            stream_epoch: next_epoch,
            predecessor: 0,
            sequence: 1,
            payload: digest_id(103),
            ..b
        };
        check_eq!(
            source_authority::admit(
                &connection,
                new_sibling,
                SourceOperationMode::Advance,
                &[b_baseline],
                &mut admission_budget(),
            ),
            Err(IndexError::Conflict)
        );
        let reset = OperationKey {
            stream_epoch: next_epoch,
            ..first
        };
        let a_baseline = SourceEvent {
            stream_epoch: next_epoch,
            payload: digest_id(104),
            ..a
        };
        let mode = SourceOperationMode::Resync {
            prior_epoch: first.stream_epoch,
        };
        source_authority::admit(
            &connection,
            reset,
            mode,
            &[a_baseline],
            &mut admission_budget(),
        )?;
        source_authority::commit(
            &connection,
            reset,
            mode,
            &[a_baseline],
            &mut admission_budget(),
        )?;
        install_frontiers(&connection, first.workspace, &[a_baseline], mode)?;
        let old_sibling = OperationKey {
            sequence: 2,
            ..sibling
        };
        let b_old_advance = SourceEvent {
            predecessor: 1,
            sequence: 2,
            payload: digest_id(105),
            ..b
        };
        check_eq!(
            source_authority::admit(
                &connection,
                old_sibling,
                SourceOperationMode::Advance,
                &[b_old_advance],
                &mut admission_budget(),
            ),
            Err(IndexError::Conflict)
        );
        source_authority::admit(
            &connection,
            new_sibling,
            SourceOperationMode::Advance,
            &[b_baseline],
            &mut admission_budget(),
        )?;
        source_authority::commit(
            &connection,
            new_sibling,
            SourceOperationMode::Advance,
            &[b_baseline],
            &mut admission_budget(),
        )?;
        install_frontiers(
            &connection,
            first.workspace,
            &[b_baseline],
            SourceOperationMode::Advance,
        )?;
        let published: (i64, i64) = connection.query_row(
            "SELECT stream_epoch,sequence FROM native_frontier_v1
             WHERE workspace=?1 AND producer=?2 AND stream=?3",
            params![
                workspace_key(first.workspace)?,
                first.producer.0.to_vec(),
                sibling.stream.0.to_vec(),
            ],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        check_eq!(published, (int(next_epoch)?, 1));
        Ok(())
    }

    #[test]
    fn failed_first_source_admission_rolls_back_epoch_seed() -> TestResult {
        let root = tempfile::tempdir()?;
        let (catalog, genesis) = setup(root.path())?;
        let operation = ImmutableOperation {
            source_mode: SourceOperationMode::Advance,
            ..operation(1, 106)
        };
        let event = SourceEvent {
            producer: operation.key.producer,
            stream: operation.key.stream,
            stream_epoch: operation.key.stream_epoch,
            predecessor: 0,
            sequence: 1,
            payload: digest_id(107),
        };
        let ingress = stage_inputs(&catalog, operation, &[object(106, vec![])])?;
        check!(matches!(
            catalog.admit(operation, ingress, &[event, event], &mut admission_budget(),),
            Err(IndexError::InvalidInput(_))
        ));
        check_eq!(
            inspect_with_test_budget(&catalog, operation.key)?,
            OperationStatus::Unknown
        );
        check_eq!(
            catalog.source_epoch(
                genesis.identity,
                operation.key.producer,
                &mut admission_budget(),
            )?,
            None
        );
        check_eq!(
            fixture_read_ingress(&catalog, operation.key)?.map(|row| row.stage),
            Some(ingress)
        );
        Ok(())
    }

    #[test]
    fn projection_event_advances_frontier_without_source_change_and_replays_receipt() -> TestResult
    {
        let root = tempfile::tempdir()?;
        let (catalog, genesis) = setup(root.path())?;
        let session = catalog.open_read_session()?;
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
        check_eq!(
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
                    source_mode: SourceOperationMode::Unchanged,
                }
            )?,
            OperationStatus::Published(next.identity)
        );
        check_eq!(next.source, genesis.source);
        drop(catalog);
        let catalog = open(root.path())?;
        check_eq!(
            inspect_with_test_budget(&catalog, first.key)?,
            OperationStatus::Published(next.identity)
        );
        check_eq!(
            catalog.admit(
                first,
                replay_ingress(first),
                &[event],
                &mut admission_budget()
            )?,
            AdmissionOutcome::Existing(OperationStatus::Published(next.identity))
        );
        let exact = acquire_ticket_with_test_budget(
            &catalog,
            session.as_ref(),
            ViewSelection::Exact(next.identity),
            Duration::from_secs(30),
        )?;
        check_eq!(exact.view, next);
        catalog.release_ticket(session.as_ref(), exact.lease)?;

        let second = operation(602, 154);
        let second_event = SourceEvent {
            predecessor: 1,
            sequence: 2,
            payload: digest_id(155),
            ..event
        };
        let second_attempt = admit_new_events(&catalog, second, &[second_event])?;
        let following = view(2, genesis.profile, None);
        check!(matches!(
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
                    source_mode: SourceOperationMode::Unchanged,
                }
            ),
            Err(IndexError::InvalidInput(_))
        ));
        check_eq!(
            inspect_with_test_budget(&catalog, second.key)?,
            OperationStatus::Pending(second_attempt)
        );
        check_eq!(
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
                    source_mode: SourceOperationMode::Unchanged,
                }
            )?,
            OperationStatus::Published(following.identity)
        );
        check_eq!(following.source, genesis.source);
        Ok(())
    }

    #[test]
    fn reserved_root_cannot_publish_and_corrupt_edge_refuses_exact_read() -> TestResult {
        let root = tempfile::tempdir()?;
        let (catalog, genesis) = setup(root.path())?;
        let session = catalog.open_read_session()?;
        let operation = operation(4, 90);
        let attempt = admit_new(&catalog, operation)?;
        let leaf = object(91, vec![]);
        reserve_confirm(&catalog, attempt, std::slice::from_ref(&leaf))?;
        // Two ordered branch slots may legitimately point to one immutable child.
        let parent = object(92, vec![leaf.reference, leaf.reference]);
        catalog.reserve_objects(attempt, std::slice::from_ref(&parent))?;
        let next = view(1, genesis.profile, Some(parent.reference));
        check!(matches!(
            publish(&catalog, attempt, &genesis, next.clone()),
            Err(IndexError::CorruptObject(_))
        ));
        check_eq!(
            inspect_with_test_budget(&catalog, operation.key)?,
            OperationStatus::Pending(attempt)
        );
        catalog.register_objects(attempt, std::slice::from_ref(&parent))?;
        check_eq!(
            publish(&catalog, attempt, &genesis, next.clone())?,
            OperationStatus::Published(next.identity)
        );

        let direct = Connection::open(catalog.path())?;
        let changed = direct.execute(
            "UPDATE native_object_edge_v1 SET row_digest=zeroblob(32) WHERE parent=?1",
            params![object_key(parent.reference)],
        )?;
        check_eq!(changed, 1);
        check!(matches!(
            acquire_ticket_with_test_budget(
                &catalog,
                session.as_ref(),
                ViewSelection::Exact(next.identity),
                Duration::from_secs(30),
            ),
            Err(IndexError::CorruptObject(_))
        ));
        Ok(())
    }

    #[test]
    fn refused_event_identity_retains_original_payload_across_rebind() -> TestResult {
        let root = tempfile::tempdir()?;
        let (catalog, genesis) = setup(root.path())?;
        let session = catalog.open_read_session()?;
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
        check_eq!(
            catalog.refuse(first_attempt, Refusal::InvalidInput)?,
            OperationStatus::Refused(Refusal::InvalidInput)
        );
        let second = operation(11, 101);
        let second_attempt = admit_new_events(&catalog, second, &[event])?;
        check_eq!(
            inspect_with_test_budget(&catalog, first.key)?,
            OperationStatus::Refused(Refusal::InvalidInput)
        );
        let competing = operation(12, 102);
        let competing_input = object(102, vec![]);
        let competing_ingress =
            stage_inputs(&catalog, competing, std::slice::from_ref(&competing_input))?;
        check_eq!(
            catalog.admit(
                competing,
                competing_ingress,
                &[event],
                &mut admission_budget()
            ),
            Err(IndexError::Conflict)
        );
        check_eq!(
            catalog.refuse(second_attempt, Refusal::Conflict)?,
            OperationStatus::Refused(Refusal::Conflict)
        );
        let changed = SourceEvent {
            payload: digest_id(105),
            ..event
        };
        check_eq!(
            catalog.admit(
                competing,
                competing_ingress,
                &[changed],
                &mut admission_budget()
            ),
            Err(IndexError::DigestConflict)
        );
        check_eq!(
            inspect_with_test_budget(&catalog, competing.key)?,
            OperationStatus::Unknown
        );
        let latest = acquire_ticket_with_test_budget(
            &catalog,
            session.as_ref(),
            ViewSelection::Latest {
                workspace: workspace(),
                epoch: 7,
            },
            Duration::from_secs(30),
        )?;
        check_eq!(latest.view, genesis);
        catalog.release_ticket(session.as_ref(), latest.lease)?;
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
        check_eq!(changed, 1);
        drop(direct);
        check!(
            matches!(open(root.path()), Err(CoreError::Storage(message)) if message.contains("counter digest mismatch"))
        );
        for checksum in [
            rusqlite::types::Value::Blob(vec![0; 4097]),
            rusqlite::types::Value::Text("01234567890123456789012345678901".into()),
        ] {
            let root = tempfile::tempdir()?;
            let (catalog, _) = setup(root.path())?;
            let path = catalog.path().to_path_buf();
            drop(catalog);
            let direct = Connection::open(&path)?;
            check_eq!(
                direct.execute(
                    "UPDATE native_counter_v1 SET row_digest=?1 WHERE name='attempt_fence'",
                    params![checksum]
                )?,
                1
            );
            drop(direct);
            check!(
                matches!(open(root.path()), Err(CoreError::Storage(message)) if message.contains("read native counter"))
            );
            let direct = Connection::open(path)?;
            check_eq!(
                direct.query_row(
                    "SELECT row_digest FROM native_counter_v1 WHERE name='attempt_fence'",
                    [],
                    |row| row.get::<_, rusqlite::types::Value>(0)
                )?,
                checksum
            );
        }
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
        check!(resumed.fence > first.fence);
        check_eq!(
            catalog.reserve_inputs(first, std::slice::from_ref(&request)),
            Err(IndexError::FenceLost)
        );
        check_eq!(
            catalog.discard_inputs(first, &mut admission_budget()),
            Err(IndexError::FenceLost)
        );
        catalog.reserve_inputs(resumed, std::slice::from_ref(&request))?;
        catalog.confirm_inputs(resumed, std::slice::from_ref(&request))?;
        catalog.seal_inputs(resumed, request.reference)?;
        check!(matches!(
            catalog.admit(operation, resumed, &[], &mut admission_budget())?,
            AdmissionOutcome::New(_)
        ));
        check!(matches!(
            inspect_with_test_budget(&catalog, operation.key)?,
            OperationStatus::Pending(_)
        ));
        check_eq!(fixture_read_ingress(&catalog, operation.key)?, None);
        let operation_key = operation_key(operation.key)?;
        let direct = Connection::open(catalog.path())?;
        let protected: i64 = direct.query_row(
            "SELECT COUNT(*) FROM native_operation_object_v1 WHERE operation=?1",
            params![operation_key],
            |row| row.get(0),
        )?;
        check_eq!(protected, 2);
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
        check!(matches!(
            catalog.reserve_inputs(ingress, &[request.clone(), leaf.clone()]),
            Err(IndexError::CorruptObject(_))
        ));
        catalog.reserve_inputs(ingress, &[leaf.clone(), request.clone()])?;
        catalog.confirm_inputs(ingress, std::slice::from_ref(&leaf))?;
        check!(matches!(
            catalog.seal_inputs(ingress, request.reference),
            Err(IndexError::InvalidInput(_))
        ));
        check_eq!(
            inspect_with_test_budget(&catalog, operation.key)?,
            OperationStatus::Unknown
        );
        catalog.confirm_inputs(ingress, std::slice::from_ref(&request))?;
        catalog.seal_inputs(ingress, request.reference)?;
        check_eq!(
            catalog.seal_inputs(ingress, leaf.reference),
            Err(IndexError::DigestConflict)
        );
        let wrong_request = ImmutableOperation {
            request: leaf.reference,
            ..operation
        };
        check_eq!(
            catalog.admit(wrong_request, ingress, &[], &mut admission_budget()),
            Err(IndexError::DigestConflict)
        );
        check_eq!(
            inspect_with_test_budget(&catalog, operation.key)?,
            OperationStatus::Unknown
        );
        check!(matches!(
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
            objects.push(RegisteredObject::unscoped(
                ObjectRef(Digest(bytes)),
                1,
                vec![],
            ));
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
        check_eq!(
            catalog.admit(operation, ingress, &[], &mut limited),
            Err(IndexError::ResourceLimited)
        );
        check_eq!(
            inspect_with_test_budget(&catalog, operation.key)?,
            OperationStatus::Unknown
        );
        check_eq!(
            fixture_read_ingress(&catalog, operation.key)?.map(|row| row.stage),
            Some(ingress)
        );
        let transferred: i64 = catalog.lock()?.query_row(
            "SELECT COUNT(*) FROM native_operation_object_v1 WHERE operation=?1",
            params![operation_key(operation.key)?],
            |row| row.get(0),
        )?;
        check_eq!(transferred, 0);

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
        check_eq!(
            catalog.admit(operation, ingress, &[], &mut cancelled),
            Err(IndexError::Cancelled)
        );
        check_eq!(
            inspect_with_test_budget(&catalog, operation.key)?,
            OperationStatus::Unknown
        );
        check!(matches!(
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
            objects.push(RegisteredObject::unscoped(
                ObjectRef(Digest(bytes)),
                1,
                vec![],
            ));
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
        check_eq!(
            catalog.discard_inputs(ingress, &mut limited),
            Err(IndexError::ResourceLimited)
        );
        check_eq!(
            fixture_read_ingress(&catalog, operation.key)?.map(|row| row.stage),
            Some(ingress)
        );
        for table in ["native_ingress_intent_v1", "native_ingress_object_v1"] {
            let query = format!("SELECT COUNT(*) FROM {table} WHERE operation=?1");
            let retained: i64 = catalog.lock()?.query_row(
                &query,
                params![operation_key(operation.key)?],
                |row| row.get(0),
            )?;
            check_eq!(retained, 258);
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
        check_eq!(
            catalog.discard_inputs(ingress, &mut cancelled),
            Err(IndexError::Cancelled)
        );
        catalog.discard_inputs(ingress, &mut admission_budget())?;
        check_eq!(fixture_read_ingress(&catalog, operation.key)?, None);
        check_eq!(
            inspect_with_test_budget(&catalog, operation.key)?,
            OperationStatus::Unknown
        );
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
        check_eq!(
            catalog.admit(operation, ingress, &[event, second_event], &mut limited),
            Err(IndexError::ResourceLimited)
        );
        check_eq!(
            inspect_with_test_budget(&catalog, operation.key)?,
            OperationStatus::Unknown
        );
        check_eq!(
            fixture_read_ingress(&catalog, operation.key)?.map(|row| row.stage),
            Some(ingress)
        );
        let claims: i64 = catalog.lock()?.query_row(
            "SELECT COUNT(*) FROM native_event_claim_v1 WHERE operation=?1",
            params![operation_key(operation.key)?],
            |row| row.get(0),
        )?;
        check_eq!(claims, 0);
        check!(matches!(
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
        check_eq!(
            catalog.admit(competing, ingress, &[event], &mut admission_budget()),
            Err(IndexError::Conflict)
        );
        check_eq!(
            inspect_with_test_budget(&catalog, competing.key)?,
            OperationStatus::Unknown
        );
        check_eq!(
            fixture_read_ingress(&catalog, competing.key)?.map(|row| row.stage),
            Some(ingress)
        );
        check_eq!(
            catalog.refuse(first_attempt, Refusal::Conflict)?,
            OperationStatus::Refused(Refusal::Conflict)
        );
        check!(matches!(
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
        check_eq!(catalog.open_ingress(stale.key), Err(IndexError::Conflict));
        check_eq!(fixture_read_ingress(&catalog, stale.key)?, None);
        Ok(())
    }

    #[test]
    fn read_session_recovery_requires_os_death_proof() -> TestResult {
        let root = tempfile::tempdir()?;
        let (catalog, genesis) = setup(root.path())?;
        let session = catalog.open_read_session()?;
        let ticket = acquire_ticket_with_test_budget(
            &catalog,
            session.as_ref(),
            ViewSelection::Exact(genesis.identity),
            Duration::from_secs(30),
        )?;
        let mut budget = admission_budget();
        let live = catalog.recover_read_sessions(None, 1, &mut budget)?;
        check_eq!(live.recovered, 0);
        check_eq!(live.pins_released, 0);
        catalog.validate_ticket(session.as_ref(), ticket.lease)?;
        drop(catalog);

        let reopened = open(root.path())?;
        // A second Catalog process object does not prove this live reader dead.
        check_eq!(
            reopened
                .recover_read_sessions(None, 1, &mut admission_budget())?
                .pins_released,
            0
        );
        drop(session);
        let dead = reopened.recover_read_sessions(None, 1, &mut admission_budget())?;
        check_eq!(dead.recovered, 1);
        check_eq!(dead.pins_released, 1);
        let direct = Connection::open(reopened.path())?;
        let remaining: i64 =
            direct.query_row("SELECT COUNT(*) FROM native_pin_v1", [], |row| row.get(0))?;
        check_eq!(remaining, 0);
        Ok(())
    }

    fn assert_ticket_acquire_mutex_wait(cancel: bool) -> TestResult {
        use std::sync::mpsc;

        let root = tempfile::tempdir()?;
        let (catalog, genesis) = setup(root.path())?;
        let identity = genesis.identity;
        let session = catalog.open_read_session()?;
        let catalog = Arc::new(catalog);
        let held = catalog.lock()?;
        let deadline = Instant::now()
            .checked_add(if cancel {
                Duration::from_secs(5)
            } else {
                Duration::from_millis(120)
            })
            .ok_or("ticket deadline overflow")?;
        let request = quanta_index_core::RequestBudgetV1::until(deadline);
        let worker_request = request.clone();
        let worker_catalog = Arc::clone(&catalog);
        let worker_session = Arc::clone(&session);
        let (started_tx, started_rx) = mpsc::channel();
        let (finished_tx, finished_rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let budget = WorkBudget::with_request(
                WorkLimits {
                    reads: 100,
                    writes: 100,
                    bytes: 4096,
                    mutations: 100,
                },
                worker_request,
            );
            let _started = started_tx.send(());
            let result = worker_catalog.acquire_ticket(
                worker_session.as_ref(),
                ViewSelection::Exact(identity),
                Duration::from_secs(30),
                &budget,
            );
            let _finished = finished_tx.send((result, budget.first_denial()));
        });
        if started_rx.recv_timeout(Duration::from_secs(1)).is_err() {
            drop(held);
            worker
                .join()
                .map_err(|panic| format!("ticket worker panicked: {panic:?}"))?;
            return Err("ticket worker did not start".into());
        }
        std::thread::sleep(Duration::from_millis(40));
        if cancel {
            request.cancel_handle().cancel();
        }
        let finished = finished_rx.recv_timeout(Duration::from_secs(2));
        drop(held);
        worker
            .join()
            .map_err(|panic| format!("ticket worker panicked: {panic:?}"))?;
        let (result, denial) = finished
            .map_err(|timeout| format!("ticket waited beyond original budget: {timeout}"))?;
        let refused_as_expected = if cancel {
            matches!(&result, Err(IndexError::Cancelled))
        } else {
            matches!(&result, Err(IndexError::ResourceLimited))
        };
        check!(refused_as_expected);
        check_eq!(
            denial,
            Some(if cancel {
                WorkBudgetDenial::Cancelled
            } else {
                WorkBudgetDenial::Deadline
            })
        );
        check_eq!(fixture_pin_count(&catalog)?, 0);
        let retry = acquire_ticket_with_test_budget(
            &catalog,
            session.as_ref(),
            ViewSelection::Exact(identity),
            Duration::from_secs(30),
        )?;
        catalog.release_ticket(session.as_ref(), retry.lease)?;
        Ok(())
    }

    #[test]
    fn read_ticket_acquire_cancelled_while_catalog_mutex_is_held() -> TestResult {
        assert_ticket_acquire_mutex_wait(true)
    }

    #[test]
    fn read_ticket_acquire_deadline_while_catalog_mutex_is_held() -> TestResult {
        assert_ticket_acquire_mutex_wait(false)
    }

    #[test]
    fn read_ticket_acquire_cancel_before_commit_rolls_back_pin() -> TestResult {
        let root = tempfile::tempdir()?;
        let stop = Arc::new(AtomicBool::new(false));
        let clock = Arc::new(CancelOnNextClock {
            now: 1_000,
            armed: AtomicBool::new(false),
            stop: Arc::clone(&stop),
        });
        let (catalog, genesis) = setup_with_clock(root.path(), clock.clone())?;
        let session = catalog.open_read_session()?;
        check_eq!(fixture_pin_count(&catalog)?, 0);
        let deadline = Instant::now()
            .checked_add(Duration::from_secs(5))
            .ok_or("ticket deadline overflow")?;
        let budget = WorkBudget::with_request(
            WorkLimits {
                reads: 100,
                writes: 100,
                bytes: 4096,
                mutations: 100,
            },
            quanta_index_core::RequestBudgetV1::until_with_external_cancellation(
                deadline,
                Arc::clone(&stop),
            ),
        );
        clock.armed.store(true, Ordering::Release);
        check_eq!(
            catalog.acquire_ticket(
                session.as_ref(),
                ViewSelection::Exact(genesis.identity),
                Duration::from_secs(30),
                &budget,
            ),
            Err(IndexError::Cancelled)
        );
        check_eq!(budget.first_denial(), Some(WorkBudgetDenial::Cancelled));
        check!(stop.load(Ordering::Acquire));
        check_eq!(fixture_pin_count(&catalog)?, 0);
        Ok(())
    }

    #[test]
    fn read_ticket_acquire_returns_committed_pin_after_commit_hook_cancels() -> TestResult {
        let root = tempfile::tempdir()?;
        let (catalog, genesis) = setup(root.path())?;
        let session = catalog.open_read_session()?;
        let stop = Arc::new(AtomicBool::new(false));
        {
            let connection = catalog.lock()?;
            let hook_stop = Arc::clone(&stop);
            connection.commit_hook(Some(move || {
                hook_stop.store(true, Ordering::Release);
                false
            }))?;
        }
        let deadline = Instant::now()
            .checked_add(Duration::from_secs(5))
            .ok_or("ticket deadline overflow")?;
        let budget = WorkBudget::with_request(
            WorkLimits {
                reads: 100,
                writes: 100,
                bytes: 4096,
                mutations: 100,
            },
            quanta_index_core::RequestBudgetV1::until_with_external_cancellation(
                deadline,
                Arc::clone(&stop),
            ),
        );
        let ticket = catalog.acquire_ticket(
            session.as_ref(),
            ViewSelection::Exact(genesis.identity),
            Duration::from_secs(30),
            &budget,
        )?;
        check!(stop.load(Ordering::Acquire));
        {
            let connection = catalog.lock()?;
            connection.commit_hook(None::<fn() -> bool>)?;
        }
        check_eq!(fixture_pin_count(&catalog)?, 1);
        catalog.validate_ticket(session.as_ref(), ticket.lease)?;
        catalog.release_ticket(session.as_ref(), ticket.lease)?;
        check_eq!(fixture_pin_count(&catalog)?, 0);
        Ok(())
    }

    #[test]
    fn read_ticket_renewal_fences_old_lease_and_foreign_guard() -> TestResult {
        use quanta_index_core::index::ReadSessionRef;

        struct ForeignSession(ReadSessionRef);
        impl quanta_index_core::index::ReadSessionLease for ForeignSession {
            fn binding(&self) -> ReadSessionRef {
                self.0
            }
        }
        let root = tempfile::tempdir()?;
        let (catalog, genesis) = setup(root.path())?;
        let session = catalog.open_read_session()?;
        let ticket = acquire_ticket_with_test_budget(
            &catalog,
            session.as_ref(),
            ViewSelection::Exact(genesis.identity),
            Duration::from_secs(30),
        )?;
        let next = catalog.renew_ticket(session.as_ref(), ticket.lease, Duration::from_secs(30))?;
        check_ne!(next.fence, ticket.lease.fence);
        check_eq!(
            catalog.validate_ticket(session.as_ref(), ticket.lease),
            Err(IndexError::SnapshotUnavailable)
        );
        check!(matches!(
            catalog.validate_ticket(&ForeignSession(session.binding()), next),
            Err(IndexError::InvalidInput(_))
        ));
        catalog.validate_ticket(session.as_ref(), next)?;
        catalog.release_ticket(session.as_ref(), next)?;
        check_eq!(
            catalog.validate_ticket(session.as_ref(), next),
            Err(IndexError::SnapshotUnavailable)
        );
        Ok(())
    }

    #[test]
    fn read_ticket_clock_boundary_expiry_and_live_session_custody() -> TestResult {
        let root = tempfile::tempdir()?;
        let clock = Arc::new(ManualClock::new(1_000));
        let (catalog, genesis) = setup_with_clock(root.path(), clock.clone())?;
        let session = catalog.open_read_session()?;
        let original = acquire_ticket_with_test_budget(
            &catalog,
            session.as_ref(),
            ViewSelection::Exact(genesis.identity),
            Duration::from_millis(10),
        )?;
        check_eq!(original.lease.expires_at_ms, 1_010);
        clock.set(1_009);
        catalog.validate_ticket(session.as_ref(), original.lease)?;
        let renewed =
            catalog.renew_ticket(session.as_ref(), original.lease, Duration::from_millis(10))?;
        check_eq!(renewed.expires_at_ms, 1_019);
        check_eq!(
            catalog.validate_ticket(session.as_ref(), original.lease),
            Err(IndexError::SnapshotUnavailable)
        );
        clock.set(1_019);
        check_eq!(
            catalog.validate_ticket(session.as_ref(), renewed),
            Err(IndexError::SnapshotUnavailable)
        );
        check_eq!(
            catalog.renew_ticket(session.as_ref(), renewed, Duration::from_millis(10)),
            Err(IndexError::SnapshotUnavailable)
        );
        let live = catalog.recover_read_sessions(None, 1, &mut admission_budget())?;
        check_eq!((live.recovered, live.pins_released), (0, 0));
        let pins: i64 =
            catalog
                .lock()?
                .query_row("SELECT COUNT(*) FROM native_pin_v1", [], |row| row.get(0))?;
        check_eq!(pins, 1);
        catalog.release_ticket(session.as_ref(), renewed)?;
        let pins: i64 =
            catalog
                .lock()?
                .query_row("SELECT COUNT(*) FROM native_pin_v1", [], |row| row.get(0))?;
        check_eq!(pins, 0);

        let pending = acquire_ticket_with_test_budget(
            &catalog,
            session.as_ref(),
            ViewSelection::Exact(genesis.identity),
            Duration::from_millis(10),
        )?;
        clock.set(1_029);
        check_eq!(
            catalog.validate_ticket(session.as_ref(), pending.lease),
            Err(IndexError::SnapshotUnavailable)
        );
        let live = catalog.recover_read_sessions(None, 1, &mut admission_budget())?;
        check_eq!((live.recovered, live.pins_released), (0, 0));
        drop(session);
        let dead = catalog.recover_read_sessions(None, 1, &mut admission_budget())?;
        check_eq!((dead.recovered, dead.pins_released), (1, 1));
        Ok(())
    }

    #[test]
    fn read_ticket_catalog_clock_regression_refuses_without_losing_pin() -> TestResult {
        let root = tempfile::tempdir()?;
        let clock = Arc::new(ManualClock::new(2_000));
        let (catalog, genesis) = setup_with_clock(root.path(), clock.clone())?;
        let session = catalog.open_read_session()?;
        let ticket = acquire_ticket_with_test_budget(
            &catalog,
            session.as_ref(),
            ViewSelection::Exact(genesis.identity),
            Duration::from_millis(10),
        )?;
        clock.set(1_999);
        check!(matches!(
            catalog.validate_ticket(session.as_ref(), ticket.lease),
            Err(IndexError::Storage(message)) if message.contains("catalog clock regressed")
        ));
        let pins: i64 =
            catalog
                .lock()?
                .query_row("SELECT COUNT(*) FROM native_pin_v1", [], |row| row.get(0))?;
        check_eq!(pins, 1);
        clock.set(2_001);
        catalog.validate_ticket(session.as_ref(), ticket.lease)?;
        catalog.release_ticket(session.as_ref(), ticket.lease)?;
        Ok(())
    }

    #[test]
    fn read_pin_digest_binds_exact_view_sequence() -> TestResult {
        let root = tempfile::tempdir()?;
        let (catalog, genesis) = setup(root.path())?;
        let operation = operation(880, 210);
        let attempt = admit_new(&catalog, operation)?;
        let next = view(1, genesis.profile, None);
        check_eq!(
            publish(&catalog, attempt, &genesis, next.clone())?,
            OperationStatus::Published(next.identity)
        );
        let session = catalog.open_read_session()?;
        let ticket = acquire_ticket_with_test_budget(
            &catalog,
            session.as_ref(),
            ViewSelection::Exact(genesis.identity),
            Duration::from_secs(30),
        )?;
        let direct = Connection::open(catalog.path())?;
        let changed = direct.execute(
            "UPDATE native_pin_v1 SET sequence=?2 WHERE ticket=?1",
            params![
                ticket_to_integer(ticket.lease.id)?,
                int(next.identity.sequence)?
            ],
        )?;
        check_eq!(changed, 1);
        check!(matches!(
            catalog.validate_ticket(session.as_ref(), ticket.lease),
            Err(IndexError::Storage(message)) if message.contains("pin digest mismatch")
        ));
        Ok(())
    }

    #[test]
    fn read_recovery_keeps_partial_session_cursor_and_retries_bounded_batch() -> TestResult {
        let root = tempfile::tempdir()?;
        let (catalog, genesis) = setup(root.path())?;
        let session = catalog.open_read_session()?;
        for _ in 0..257 {
            let ticket = acquire_ticket_with_test_budget(
                &catalog,
                session.as_ref(),
                ViewSelection::Exact(genesis.identity),
                Duration::from_secs(30),
            )?;
            check_eq!(ticket.view, genesis);
        }
        drop(session);
        let first = catalog.recover_read_sessions(None, 1, &mut admission_budget())?;
        check_eq!(
            (
                first.recovered,
                first.pins_released,
                first.after,
                first.has_more
            ),
            (0, 256, None, true)
        );
        let remaining: i64 =
            catalog
                .lock()?
                .query_row("SELECT COUNT(*) FROM native_pin_v1", [], |row| row.get(0))?;
        check_eq!(remaining, 1);
        let second = catalog.recover_read_sessions(first.after, 1, &mut admission_budget())?;
        check_eq!(
            (second.recovered, second.pins_released, second.has_more),
            (1, 1, false)
        );
        Ok(())
    }

    #[test]
    fn read_recovery_budget_exhaustion_rolls_back_whole_session_batch() -> TestResult {
        let root = tempfile::tempdir()?;
        let (catalog, genesis) = setup(root.path())?;
        let session = catalog.open_read_session()?;
        for _ in 0..2 {
            let ticket = acquire_ticket_with_test_budget(
                &catalog,
                session.as_ref(),
                ViewSelection::Exact(genesis.identity),
                Duration::from_secs(30),
            )?;
            check_eq!(ticket.view, genesis);
        }
        drop(session);
        let mut exhausted = WorkBudget::new(
            WorkLimits {
                reads: 100,
                writes: 1,
                bytes: 0,
                mutations: 0,
            },
            Instant::now() + Duration::from_secs(30),
        );
        check_eq!(
            catalog.recover_read_sessions(None, 1, &mut exhausted),
            Err(IndexError::ResourceLimited)
        );
        let remaining: i64 =
            catalog
                .lock()?
                .query_row("SELECT COUNT(*) FROM native_pin_v1", [], |row| row.get(0))?;
        check_eq!(remaining, 2);
        let retried = catalog.recover_read_sessions(None, 1, &mut admission_budget())?;
        check_eq!((retried.recovered, retried.pins_released), (1, 2));
        Ok(())
    }

    #[test]
    fn read_session_child_sigkill_recovery() -> TestResult {
        struct ChildGuard(std::process::Child);
        impl Drop for ChildGuard {
            fn drop(&mut self) {
                drop(self.0.kill());
                drop(self.0.wait());
            }
        }
        if let Some(root) = std::env::var_os("QIDX_NATIVE_READ_CHILD_ROOT") {
            let root = Path::new(&root);
            let catalog = open(root)?;
            let session = catalog.open_read_session()?;
            let _ticket = acquire_ticket_with_test_budget(
                &catalog,
                session.as_ref(),
                ViewSelection::Latest {
                    workspace: workspace(),
                    epoch: 7,
                },
                Duration::from_secs(30),
            )?;
            std::fs::write(root.join("child-ready"), b"ready")?;
            loop {
                std::thread::park_timeout(Duration::from_secs(1));
            }
        }

        let root = tempfile::tempdir()?;
        let (catalog, _) = setup(root.path())?;
        let executable = std::env::current_exe()?;
        let mut child = ChildGuard(
            std::process::Command::new(executable)
                .arg("read_session_child_sigkill_recovery")
                .arg("--test-threads=1")
                .env("QIDX_NATIVE_READ_CHILD_ROOT", root.path())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()?,
        );
        let ready = root.path().join("child-ready");
        for _ in 0..250 {
            if ready.exists() {
                break;
            }
            if child.0.try_wait()?.is_some() {
                return Err("read session child exited before readiness".into());
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        if !ready.exists() {
            return Err("read session child did not become ready".into());
        }
        let live = catalog.recover_read_sessions(None, 1, &mut admission_budget())?;
        check_eq!(live.pins_released, 0);
        child.0.kill()?;
        let status = child.0.wait()?;
        check!(!status.success());
        let dead = catalog.recover_read_sessions(None, 1, &mut admission_budget())?;
        check_eq!((dead.recovered, dead.pins_released), (1, 1));
        Ok(())
    }

    #[test]
    fn read_session_open_crash_prefixes_recover_without_directory_scan() -> TestResult {
        struct ChildGuard(std::process::Child);
        impl Drop for ChildGuard {
            fn drop(&mut self) {
                drop(self.0.kill());
                drop(self.0.wait());
            }
        }
        if let Some(root) = std::env::var_os("QIDX_NATIVE_OPEN_CHILD_ROOT") {
            let catalog = open(Path::new(&root))?;
            let _session = catalog.open_read_session()?;
            return Err("read session child did not reach crash failpoint".into());
        }
        for (phase, expected_intents, expected_sessions, expected_file) in [
            ("intent", 1, 0, false),
            ("file", 1, 0, true),
            ("session", 0, 1, true),
        ] {
            let root = tempfile::tempdir()?;
            let (catalog, _genesis) = setup(root.path())?;
            let mut child = ChildGuard(
                std::process::Command::new(std::env::current_exe()?)
                    .arg("read_session_open_crash_prefixes_recover_without_directory_scan")
                    .arg("--test-threads=1")
                    .env("QIDX_NATIVE_OPEN_CHILD_ROOT", root.path())
                    .env("QIDX_NATIVE_READ_OPEN_CRASH", phase)
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn()?,
            );
            let ready = root.path().join("open-phase-ready");
            for _ in 0..250 {
                if ready.exists() {
                    break;
                }
                if child.0.try_wait()?.is_some() {
                    return Err(
                        format!("read session child exited before {phase} failpoint").into(),
                    );
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            if !ready.exists() {
                return Err(format!("read session child did not reach {phase} failpoint").into());
            }
            check_eq!(std::fs::read_to_string(&ready)?, phase);
            child.0.kill()?;
            let status = child.0.wait()?;
            check!(!status.success(), "child unexpectedly survived {phase}");
            let connection = Connection::open(catalog.path())?;
            let intents: i64 = connection.query_row(
                "SELECT COUNT(*) FROM native_read_session_open_v1",
                [],
                |row| row.get(0),
            )?;
            let sessions: i64 =
                connection.query_row("SELECT COUNT(*) FROM native_read_session_v1", [], |row| {
                    row.get(0)
                })?;
            check_eq!((intents, sessions), (expected_intents, expected_sessions));
            let id: i64 = connection.query_row(
                if expected_intents == 1 {
                    "SELECT id FROM native_read_session_open_v1"
                } else {
                    "SELECT id FROM native_read_session_v1"
                },
                [],
                |row| row.get(0),
            )?;
            let path = crate::connection::catalog_dir(root.path())
                .join("native-read-sessions")
                .join(format!("{:016x}.lease", uint(id)?));
            check_eq!(path.exists(), expected_file);
            let recovered = catalog.recover_read_sessions(None, 1, &mut admission_budget())?;
            check_eq!(
                recovered.open_intents_reaped,
                u32::try_from(expected_intents)?
            );
            check_eq!(recovered.recovered, u32::try_from(expected_sessions)?);
            check!(!path.exists());
            let remaining: i64 = connection.query_row(
                "SELECT COUNT(*) FROM native_read_session_open_v1",
                [],
                |row| row.get(0),
            )?;
            check_eq!(remaining, 0);
        }
        Ok(())
    }

    #[test]
    fn read_session_open_intents_are_bounded_and_budget_atomic() -> TestResult {
        let root = tempfile::tempdir()?;
        let (catalog, _genesis) = setup(root.path())?;
        let connection = Connection::open(catalog.path())?;
        for id in 10_000_i64..10_257_i64 {
            let binding = quanta_index_core::index::ReadSessionRef {
                id: uint(id)?,
                fence: 1,
            };
            let inserted = connection.execute(
                "INSERT INTO native_read_session_open_v1(id,fence,row_digest) VALUES (?1,?2,?3)",
                params![id, 1_i64, lifetime::creation::open_intent_digest(binding)?],
            )?;
            check_eq!(inserted, 1);
        }
        let mut exhausted = WorkBudget::new(
            WorkLimits {
                reads: 1_000,
                writes: 1,
                bytes: 0,
                mutations: 0,
            },
            Instant::now() + Duration::from_secs(30),
        );
        check_eq!(
            catalog.recover_read_sessions(None, 1, &mut exhausted),
            Err(IndexError::ResourceLimited)
        );
        let remaining: i64 = connection.query_row(
            "SELECT COUNT(*) FROM native_read_session_open_v1",
            [],
            |row| row.get(0),
        )?;
        check_eq!(remaining, 257);
        let first = catalog.recover_read_sessions(None, 1, &mut admission_budget())?;
        check_eq!(first.open_intents_reaped, 256);
        check!(first.has_more);
        let second = catalog.recover_read_sessions(first.after, 1, &mut admission_budget())?;
        check_eq!(second.open_intents_reaped, 1);
        check!(!second.has_more);
        Ok(())
    }

    #[test]
    fn read_session_open_intent_foreign_symlink_fails_closed() -> TestResult {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir()?;
        let (catalog, _genesis) = setup(root.path())?;
        let _live_session = catalog.open_read_session()?;
        let binding = quanta_index_core::index::ReadSessionRef {
            id: 10_000,
            fence: 1,
        };
        let connection = Connection::open(catalog.path())?;
        let inserted = connection.execute(
            "INSERT INTO native_read_session_open_v1(id,fence,row_digest) VALUES (?1,?2,?3)",
            params![
                int(binding.id)?,
                int(binding.fence)?,
                lifetime::creation::open_intent_digest(binding)?
            ],
        )?;
        check_eq!(inserted, 1);
        let external = root.path().join("foreign");
        std::fs::write(&external, b"foreign")?;
        let pathname = crate::connection::catalog_dir(root.path())
            .join("native-read-sessions")
            .join(format!("{:016x}.lease", binding.id));
        symlink(&external, &pathname)?;
        check!(matches!(
            catalog.recover_read_sessions(None, 1, &mut admission_budget()),
            Err(IndexError::Storage(_))
        ));
        let remaining: i64 = connection.query_row(
            "SELECT COUNT(*) FROM native_read_session_open_v1",
            [],
            |row| row.get(0),
        )?;
        check_eq!(remaining, 1);
        check!(pathname.is_symlink());
        Ok(())
    }

    #[test]
    fn read_session_creator_and_sweeper_share_creation_lock() -> TestResult {
        let root = tempfile::tempdir()?;
        let (catalog, _genesis) = setup(root.path())?;
        let session = catalog.open_read_session()?;
        let path = crate::connection::catalog_dir(root.path())
            .join("native-read-sessions")
            .join(".creation.lease");
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)?;
        file.try_lock()?;
        check!(matches!(
            catalog.open_read_session(),
            Err(IndexError::ResourceLimited)
        ));
        let deferred = catalog.recover_read_sessions(None, 1, &mut admission_budget())?;
        check_eq!(deferred.recovered, 0);
        check!(deferred.has_more);
        drop(file);
        drop(session);
        let recovered = catalog.recover_read_sessions(None, 1, &mut admission_budget())?;
        check_eq!(recovered.recovered, 1);
        let next = catalog.open_read_session()?;
        check_ne!(
            next.binding().id,
            recovered.after.ok_or("missing cursor")?.id
        );
        Ok(())
    }

    fn terminal_custody_counts(
        catalog: &SqliteCatalog,
        key: OperationKey,
    ) -> Result<(i64, i64, i64), Box<dyn Error>> {
        let connection = Connection::open(catalog.path())?;
        let operation = operation_key(key)?;
        let count = |table: &str| -> Result<i64, Box<dyn Error>> {
            let query = format!("SELECT COUNT(*) FROM {table} WHERE operation=?1");
            Ok(connection.query_row(&query, params![operation], |row| row.get(0))?)
        };
        Ok((
            count("native_operation_object_v1")?,
            count("native_attempt_object_v1")?,
            count("native_object_intent_v1")?,
        ))
    }

    #[test]
    fn terminal_custody_refuses_pending_and_invalid_page_without_mutation() -> TestResult {
        let root = tempfile::tempdir()?;
        let (catalog, _genesis) = setup(root.path())?;
        let operation = operation(901, 101);
        let attempt = admit_new(&catalog, operation)?;
        let protected = object(102, vec![]);
        catalog.reserve_objects(attempt, std::slice::from_ref(&protected))?;
        let before = terminal_custody_counts(&catalog, operation.key)?;
        check_eq!(before, (1, 0, 1));

        check_eq!(
            catalog.release_terminal_custody(operation.key, 1, &mut admission_budget()),
            Err(IndexError::Conflict)
        );
        for invalid_limit in [0, 257] {
            check!(matches!(
                catalog.release_terminal_custody(
                    operation.key,
                    invalid_limit,
                    &mut admission_budget()
                ),
                Err(IndexError::InvalidInput(_))
            ));
        }
        check_eq!(terminal_custody_counts(&catalog, operation.key)?, before);
        check_eq!(
            inspect_with_test_budget(&catalog, operation.key)?,
            OperationStatus::Pending(attempt)
        );
        Ok(())
    }

    #[test]
    fn terminal_custody_ready_set_is_bounded_partial_restart_safe_and_preserves_replay()
    -> TestResult {
        let root = tempfile::tempdir()?;
        let (catalog, _genesis) = setup(root.path())?;
        let first = operation(993, 181);
        let second = operation(994, 182);
        let pending = admit_new(&catalog, first)?;
        check_eq!(
            catalog.next_terminal_custody(workspace(), 7, None, None, &mut admission_budget())?,
            None
        );
        let objects = [object(183, vec![]), object(184, vec![])];
        catalog.reserve_objects(pending, &objects)?;
        check_eq!(
            catalog.refuse(pending, Refusal::Conflict)?,
            OperationStatus::Refused(Refusal::Conflict)
        );
        let other = admit_new(&catalog, second)?;
        let _terminal = catalog.refuse(other, Refusal::Conflict)?;
        let first_ready = catalog
            .next_terminal_custody(workspace(), 7, None, None, &mut admission_budget())?
            .ok_or("missing terminal ready entry")?;
        check_eq!(first_ready.operation, first.key);
        let page =
            catalog.release_terminal_custody(first_ready.operation, 1, &mut admission_budget())?;
        check_eq!(page.released, 1);
        check!(page.has_more);
        drop(catalog);
        let catalog = SqliteCatalog::open(root.path(), Duration::from_secs(2))?;
        check_eq!(
            catalog.next_terminal_custody(
                workspace(),
                7,
                None,
                Some(first_ready.sequence),
                &mut admission_budget()
            )?,
            Some(first_ready)
        );
        let page = catalog.release_terminal_custody(first.key, 32, &mut admission_budget())?;
        check!(!page.has_more);
        check_eq!(
            catalog.next_terminal_custody(
                workspace(),
                7,
                None,
                Some(first_ready.sequence),
                &mut admission_budget()
            )?,
            None
        );
        check_eq!(
            catalog
                .next_terminal_custody(
                    workspace(),
                    7,
                    Some(first_ready.sequence),
                    None,
                    &mut admission_budget()
                )?
                .map(|candidate| candidate.operation),
            Some(second.key)
        );
        check_eq!(
            inspect_with_test_budget(&catalog, first.key)?,
            OperationStatus::Refused(Refusal::Conflict)
        );
        check_eq!(catalog.operation(first.key)?, first);
        Ok(())
    }

    #[test]
    fn terminal_custody_ready_sequence_survives_restart_and_fences_late_lower_operation()
    -> TestResult {
        let root = tempfile::tempdir()?;
        let (catalog, _genesis) = setup(root.path())?;
        let first = operation(200, 191);
        let later_lower = operation(1, 192);
        check!(operation_key(later_lower.key)? < operation_key(first.key)?);
        let attempt = admit_new(&catalog, first)?;
        let _terminal = catalog.refuse(attempt, Refusal::Conflict)?;
        let frozen = catalog
            .next_terminal_custody(workspace(), 7, None, None, &mut admission_budget())?
            .ok_or("missing first terminal-ready entry")?;
        drop(catalog);
        let catalog = SqliteCatalog::open(root.path(), Duration::from_secs(2))?;
        let attempt = admit_new(&catalog, later_lower)?;
        let _terminal = catalog.refuse(attempt, Refusal::Conflict)?;
        let next = catalog
            .next_terminal_custody(
                workspace(),
                7,
                Some(frozen.sequence),
                None,
                &mut admission_budget(),
            )?
            .ok_or("late lower operation did not enter the forward queue")?;
        check_eq!(next.operation, later_lower.key);
        check!(next.sequence > frozen.sequence);
        check_eq!(
            catalog.next_terminal_custody(
                workspace(),
                7,
                Some(frozen.sequence),
                Some(frozen.sequence),
                &mut admission_budget()
            )?,
            None
        );
        check_eq!(
            catalog.next_terminal_custody(
                workspace(),
                7,
                None,
                Some(frozen.sequence),
                &mut admission_budget()
            )?,
            Some(frozen)
        );
        Ok(())
    }

    #[test]
    fn terminal_custody_ready_accepts_widest_canonical_key_without_a_guessed_byte_cap() -> TestResult
    {
        let root = tempfile::tempdir()?;
        let (catalog, initial) = setup(root.path())?;
        let mut key = terminal_custody::widest_operation_key();
        key.epoch = i64::MAX.unsigned_abs();
        let encoded = operation_key(key)?;
        check!(encoded.len() > 512);
        let mut genesis = initial;
        genesis.identity.workspace = key.workspace;
        genesis.identity.epoch = key.epoch;
        genesis.identity.digest = genesis.identity_digest()?;
        let profile = object(30, vec![]);
        let stage = catalog.begin_bootstrap(
            key.workspace,
            key.epoch,
            &GenesisIdentity(profile.clone()),
            &mut admission_budget(),
        )?;
        catalog.reserve_bootstrap(stage, &profile)?;
        catalog.confirm_bootstrap(stage, &profile)?;
        let _created = catalog.publish_bootstrap(stage, genesis, &mut admission_budget())?;
        let attempt = admit_new(
            &catalog,
            ImmutableOperation {
                key,
                request: ObjectRef(digest_id(189)),
                source_mode: SourceOperationMode::Unchanged,
            },
        )?;
        let _terminal = catalog.refuse(attempt, Refusal::Conflict)?;
        check_eq!(
            catalog
                .next_terminal_custody(
                    key.workspace,
                    key.epoch,
                    None,
                    None,
                    &mut admission_budget()
                )?
                .map(|candidate| candidate.operation),
            Some(key)
        );
        let mut denied = WorkBudget::with_request(
            WorkLimits {
                reads: 100_000,
                writes: 100_000,
                bytes: 32 * 1024 * 1024,
                mutations: 0,
            },
            quanta_index_core::RequestBudgetV1::for_duration(Duration::from_secs(30)),
        );
        check!(matches!(
            catalog.release_terminal_custody(key, 32, &mut denied),
            Err(IndexError::ResourceLimited)
        ));
        check_eq!(
            catalog
                .next_terminal_custody(
                    key.workspace,
                    key.epoch,
                    None,
                    None,
                    &mut admission_budget()
                )?
                .map(|candidate| candidate.operation),
            Some(key)
        );
        let page = catalog.release_terminal_custody(key, 32, &mut admission_budget())?;
        check!(!page.has_more);
        check_eq!(
            catalog.next_terminal_custody(
                key.workspace,
                key.epoch,
                None,
                None,
                &mut admission_budget()
            )?,
            None
        );
        Ok(())
    }

    #[test]
    fn terminal_custody_ready_corruption_refuses_before_release() -> TestResult {
        let root = tempfile::tempdir()?;
        let (catalog, _genesis) = setup(root.path())?;
        let original = operation(995, 185);
        let pending = admit_new(&catalog, original)?;
        let _terminal = catalog.refuse(pending, Refusal::Conflict)?;
        let before = terminal_custody_counts(&catalog, original.key)?;
        let connection = Connection::open(catalog.path())?;
        let _changed = connection.execute(
            "UPDATE native_terminal_custody_v1 SET row_digest=zeroblob(32) WHERE operation=?1",
            params![operation_key(original.key)?],
        )?;
        drop(connection);
        check!(matches!(
            catalog.next_terminal_custody(workspace(), 7, None, None, &mut admission_budget()),
            Err(IndexError::Storage(_))
        ));
        check_eq!(terminal_custody_counts(&catalog, original.key)?, before);
        Ok(())
    }

    #[test]
    fn terminal_custody_ready_orphan_refuses_as_corruption_instead_of_deferred_scope() -> TestResult
    {
        let root = tempfile::tempdir()?;
        let (catalog, _genesis) = setup(root.path())?;
        let original = operation(996, 190);
        let attempt = admit_new(&catalog, original)?;
        let _terminal = catalog.refuse(attempt, Refusal::Conflict)?;
        let connection = Connection::open(catalog.path())?;
        connection.pragma_update(None, "foreign_keys", false)?;
        let _changed = connection.execute(
            "DELETE FROM native_operation_v1 WHERE operation=?1",
            params![operation_key(original.key)?],
        )?;
        drop(connection);
        check!(matches!(
            catalog.next_terminal_custody(workspace(), 7, None, None, &mut admission_budget()),
            Err(IndexError::Storage(_))
        ));
        Ok(())
    }

    #[test]
    fn terminal_custody_ready_queries_use_scope_and_key_ranges_without_history_scan() -> TestResult
    {
        let root = tempfile::tempdir()?;
        let (catalog, _genesis) = setup(root.path())?;
        let connection = Connection::open(catalog.path())?;
        for upper in [None, Some(3_i64)] {
            let query = upper.map_or(terminal_custody::READY_AFTER_QUERY, |_| {
                terminal_custody::READY_PREFIX_QUERY
            });
            let query = format!("EXPLAIN QUERY PLAN {query}");
            let mut statement = connection.prepare(&query)?;
            let mut values = vec![
                rusqlite::types::Value::Blob(workspace_key(workspace())?),
                rusqlite::types::Value::Integer(7),
                rusqlite::types::Value::Integer(2),
            ];
            if let Some(upper) = upper {
                values.push(rusqlite::types::Value::Integer(upper));
            }
            let details = statement
                .query_map(rusqlite::params_from_iter(values.iter()), |row| {
                    row.get::<_, String>(3)
                })?
                .collect::<Result<Vec<_>, _>>()?;
            check!(details.iter().any(|detail| {
                detail.contains("native_terminal_custody_scope_idx_v1")
                    && detail.contains("ready_sequence>?")
                    && (upper.is_none() || detail.contains("ready_sequence<?"))
            }));
            check!(
                !details
                    .iter()
                    .any(|detail| detail.contains("SCAN") || detail.contains("TEMP B-TREE"))
            );
        }
        Ok(())
    }

    #[test]
    fn exact_tail_policy_is_atomic_monotone_and_preserves_existing_pin() -> TestResult {
        let root = tempfile::tempdir()?;
        let (catalog, genesis) = setup(root.path())?;
        let session = catalog.open_read_session()?;
        let pin = acquire_ticket_with_test_budget(
            &catalog,
            session.as_ref(),
            ViewSelection::Exact(genesis.identity),
            Duration::from_secs(30),
        )?;
        let one = publish_plain_next(&catalog, &genesis, 1, 186)?;
        let two = publish_plain_next(&catalog, &one, 2, 187)?;
        let _three = publish_plain_next(&catalog, &two, 3, 188)?;
        check_eq!(
            catalog.retain_exact_history_tail(workspace(), 7, 2, &mut admission_budget())?,
            2
        );
        let mut unchanged_floor_budget = WorkBudget::with_request(
            WorkLimits {
                reads: 100_000,
                writes: 0,
                bytes: 32 * 1024 * 1024,
                mutations: 0,
            },
            quanta_index_core::RequestBudgetV1::for_duration(Duration::from_secs(30)),
        );
        check_eq!(
            catalog.retain_exact_history_tail(workspace(), 7, 10, &mut unchanged_floor_budget)?,
            2
        );
        check_eq!(unchanged_floor_budget.usage().writes, 0);
        check_eq!(unchanged_floor_budget.usage().mutations, 0);
        check!(matches!(
            catalog.retain_exact_history_tail(workspace(), 7, 0, &mut admission_budget()),
            Err(IndexError::InvalidInput(_))
        ));
        check_eq!(catalog.exact_history_floor(workspace(), 7)?, 2);
        catalog.validate_ticket(session.as_ref(), pin.lease)?;
        check!(matches!(
            acquire_ticket_with_test_budget(
                &catalog,
                session.as_ref(),
                ViewSelection::Exact(genesis.identity),
                Duration::from_secs(30)
            ),
            Err(IndexError::SnapshotUnavailable)
        ));
        let page =
            catalog.retire_exact_history_page(workspace(), 7, None, 32, &mut admission_budget())?;
        check_eq!((page.retired, page.blocked), (1, 1));
        catalog.validate_ticket(session.as_ref(), pin.lease)?;
        catalog.release_ticket(session.as_ref(), pin.lease)?;
        Ok(())
    }

    #[test]
    fn terminal_custody_release_is_bounded_and_preserves_views_and_replay() -> TestResult {
        let root = tempfile::tempdir()?;
        let (catalog, genesis) = setup(root.path())?;
        let session = catalog.open_read_session()?;
        let old_pin = acquire_ticket_with_test_budget(
            &catalog,
            session.as_ref(),
            ViewSelection::Exact(genesis.identity),
            Duration::from_secs(30),
        )?;
        let operation = operation(902, 103);
        let attempt = admit_new(&catalog, operation)?;
        let objects: Vec<_> = (0..129_u8)
            .map(|number| {
                let mut digest = [0_u8; 32];
                digest[0] = number;
                digest[1] = 104;
                RegisteredObject::unscoped(ObjectRef(Digest(digest)), 1, vec![])
            })
            .collect();
        catalog.reserve_objects(attempt, &objects)?;
        catalog.register_objects(attempt, &objects)?;
        check_eq!(
            terminal_custody_counts(&catalog, operation.key)?,
            (1, 129, 129)
        );
        let next = view(1, genesis.profile, None);
        check_eq!(
            publish(&catalog, attempt, &genesis, next.clone())?,
            OperationStatus::Published(next.identity)
        );

        let first =
            catalog.release_terminal_custody(operation.key, 256, &mut admission_budget())?;
        check_eq!(first.released, 256);
        check!(first.has_more);
        let second =
            catalog.release_terminal_custody(operation.key, 256, &mut admission_budget())?;
        check_eq!(second.released, 3);
        check!(!second.has_more);
        let third =
            catalog.release_terminal_custody(operation.key, 256, &mut admission_budget())?;
        check_eq!(third.released, 0);
        check!(!third.has_more);
        check_eq!(terminal_custody_counts(&catalog, operation.key)?, (0, 0, 0));

        check_eq!(
            inspect_with_test_budget(&catalog, operation.key)?,
            OperationStatus::Published(next.identity)
        );
        check_eq!(catalog.operation(operation.key)?, operation);
        check_eq!(
            catalog.admit(
                operation,
                replay_ingress(operation),
                &[],
                &mut admission_budget()
            )?,
            AdmissionOutcome::Existing(OperationStatus::Published(next.identity))
        );
        check_eq!(
            catalog.reserve_objects(attempt, objects.get(..1).ok_or("missing first object")?),
            Err(IndexError::FenceLost)
        );
        check_eq!(
            catalog.register_objects(attempt, objects.get(..1).ok_or("missing first object")?),
            Err(IndexError::FenceLost)
        );
        let connection = Connection::open(catalog.path())?;
        let registered: i64 = connection.query_row(
            "SELECT COUNT(*) FROM native_object_v1 WHERE object=?1",
            params![object_key(
                objects.first().ok_or("missing first object")?.reference
            )],
            |row| row.get(0),
        )?;
        check_eq!(registered, 1);
        catalog.validate_ticket(session.as_ref(), old_pin.lease)?;
        let new_pin = acquire_ticket_with_test_budget(
            &catalog,
            session.as_ref(),
            ViewSelection::Exact(next.identity),
            Duration::from_secs(30),
        )?;
        check_eq!(old_pin.view, genesis);
        check_eq!(new_pin.view, next);
        catalog.release_ticket(session.as_ref(), old_pin.lease)?;
        catalog.release_ticket(session.as_ref(), new_pin.lease)?;
        Ok(())
    }

    #[test]
    fn terminal_custody_budget_failure_rolls_back_and_refusal_replays() -> TestResult {
        let root = tempfile::tempdir()?;
        let (catalog, _genesis) = setup(root.path())?;
        let operation = operation(903, 105);
        let event = SourceEvent {
            producer: operation.key.producer,
            stream: operation.key.stream,
            stream_epoch: operation.key.stream_epoch,
            predecessor: 0,
            sequence: 1,
            payload: digest_id(109),
        };
        let attempt = admit_new_events(&catalog, operation, &[event])?;
        let protected = object(106, vec![]);
        catalog.reserve_objects(attempt, std::slice::from_ref(&protected))?;
        catalog.register_objects(attempt, std::slice::from_ref(&protected))?;
        check_eq!(
            catalog.refuse(attempt, Refusal::InvalidInput)?,
            OperationStatus::Refused(Refusal::InvalidInput)
        );
        let before = terminal_custody_counts(&catalog, operation.key)?;
        check_eq!(before, (1, 1, 1));
        let mut exhausted = WorkBudget::new(
            WorkLimits {
                reads: 100,
                writes: 1,
                bytes: 1 << 20,
                mutations: 100,
            },
            Instant::now() + Duration::from_secs(30),
        );
        check_eq!(
            catalog.release_terminal_custody(operation.key, 2, &mut exhausted),
            Err(IndexError::ResourceLimited)
        );
        check_eq!(terminal_custody_counts(&catalog, operation.key)?, before);
        check_eq!(
            catalog
                .release_terminal_custody(operation.key, 2, &mut admission_budget())?
                .released,
            2
        );
        check_eq!(
            catalog
                .release_terminal_custody(operation.key, 2, &mut admission_budget())?
                .released,
            1
        );
        check_eq!(terminal_custody_counts(&catalog, operation.key)?, (0, 0, 0));
        let connection = Connection::open(catalog.path())?;
        let claims: i64 = connection.query_row(
            "SELECT COUNT(*) FROM native_event_claim_v1 WHERE operation=?1",
            params![operation_key(operation.key)?],
            |row| row.get(0),
        )?;
        check_eq!(claims, 1);
        check_eq!(
            catalog.admit(
                operation,
                replay_ingress(operation),
                &[event],
                &mut admission_budget()
            )?,
            AdmissionOutcome::Existing(OperationStatus::Refused(Refusal::InvalidInput))
        );
        let changed = SourceEvent {
            payload: digest_id(110),
            ..event
        };
        check_eq!(
            catalog.admit(
                operation,
                replay_ingress(operation),
                &[changed],
                &mut admission_budget()
            ),
            Err(IndexError::DigestConflict)
        );
        Ok(())
    }

    #[test]
    fn terminal_custody_corrupt_lookahead_rolls_back_prior_deletes() -> TestResult {
        let root = tempfile::tempdir()?;
        let (catalog, _genesis) = setup(root.path())?;
        let operation = operation(904, 107);
        let attempt = admit_new(&catalog, operation)?;
        let protected = object(108, vec![]);
        catalog.reserve_objects(attempt, std::slice::from_ref(&protected))?;
        catalog.register_objects(attempt, std::slice::from_ref(&protected))?;
        check_eq!(
            catalog.refuse(attempt, Refusal::InvalidInput)?,
            OperationStatus::Refused(Refusal::InvalidInput)
        );
        let before = terminal_custody_counts(&catalog, operation.key)?;
        check_eq!(before, (1, 1, 1));
        let connection = Connection::open(catalog.path())?;
        check_eq!(
            connection.execute(
                "UPDATE native_object_intent_v1 SET row_digest=?1 WHERE operation=?2",
                params![vec![0_u8; 32], operation_key(operation.key)?],
            )?,
            1
        );
        drop(connection);
        check!(matches!(
            catalog.release_terminal_custody(operation.key, 2, &mut admission_budget()),
            Err(IndexError::Storage(_))
        ));
        check_eq!(terminal_custody_counts(&catalog, operation.key)?, before);
        check_eq!(
            inspect_with_test_budget(&catalog, operation.key)?,
            OperationStatus::Refused(Refusal::InvalidInput)
        );
        Ok(())
    }

    #[test]
    fn exact_history_floor_is_monotonic_and_preserves_existing_pin_and_receipt() -> TestResult {
        let root = tempfile::tempdir()?;
        let (catalog, genesis) = setup(root.path())?;
        check_eq!(catalog.exact_history_floor(workspace(), 7)?, 0);
        let session = catalog.open_read_session()?;
        let old_pin = acquire_ticket_with_test_budget(
            &catalog,
            session.as_ref(),
            ViewSelection::Exact(genesis.identity),
            Duration::from_secs(30),
        )?;
        let first = operation(905, 111);
        let first_attempt = admit_new(&catalog, first)?;
        let one = view(1, genesis.profile, None);
        check_eq!(
            publish(&catalog, first_attempt, &genesis, one.clone())?,
            OperationStatus::Published(one.identity)
        );
        let second = operation(906, 112);
        let second_attempt = admit_new(&catalog, second)?;
        let two = view(2, genesis.profile, None);
        check_eq!(
            publish(&catalog, second_attempt, &one, two.clone())?,
            OperationStatus::Published(two.identity)
        );
        check_eq!(
            catalog.advance_exact_history_floor(workspace(), 7, 2, &mut admission_budget())?,
            2
        );
        check_eq!(catalog.exact_history_floor(workspace(), 7)?, 2);
        check_eq!(
            catalog.advance_exact_history_floor(workspace(), 7, 2, &mut admission_budget())?,
            2
        );
        check_eq!(
            catalog.advance_exact_history_floor(workspace(), 7, 1, &mut admission_budget()),
            Err(IndexError::Conflict)
        );
        check!(matches!(
            catalog.advance_exact_history_floor(workspace(), 7, 3, &mut admission_budget()),
            Err(IndexError::InvalidInput(_))
        ));
        check_eq!(catalog.exact_history_floor(workspace(), 7)?, 2);
        for identity in [genesis.identity, one.identity] {
            check!(matches!(
                acquire_ticket_with_test_budget(
                    &catalog,
                    session.as_ref(),
                    ViewSelection::Exact(identity),
                    Duration::from_secs(30),
                ),
                Err(IndexError::SnapshotUnavailable)
            ));
        }
        catalog.validate_ticket(session.as_ref(), old_pin.lease)?;
        check_eq!(old_pin.view, genesis);
        let current = acquire_ticket_with_test_budget(
            &catalog,
            session.as_ref(),
            ViewSelection::Exact(two.identity),
            Duration::from_secs(30),
        )?;
        check_eq!(current.view, two);
        catalog.release_ticket(session.as_ref(), old_pin.lease)?;
        catalog.release_ticket(session.as_ref(), current.lease)?;
        drop(session);
        drop(catalog);

        let catalog = open(root.path())?;
        check_eq!(catalog.exact_history_floor(workspace(), 7)?, 2);
        let session = catalog.open_read_session()?;
        check!(matches!(
            acquire_ticket_with_test_budget(
                &catalog,
                session.as_ref(),
                ViewSelection::Exact(one.identity),
                Duration::from_secs(30),
            ),
            Err(IndexError::SnapshotUnavailable)
        ));
        check_eq!(
            catalog.admit(first, replay_ingress(first), &[], &mut admission_budget())?,
            AdmissionOutcome::Existing(OperationStatus::Published(one.identity))
        );
        let latest = acquire_ticket_with_test_budget(
            &catalog,
            session.as_ref(),
            ViewSelection::Latest {
                workspace: workspace(),
                epoch: 7,
            },
            Duration::from_secs(30),
        )?;
        check_eq!(latest.view, two);
        catalog.release_ticket(session.as_ref(), latest.lease)?;
        Ok(())
    }

    #[test]
    fn exact_history_floor_requires_current_epoch_and_isolates_workspaces() -> TestResult {
        let root = tempfile::tempdir()?;
        let (catalog, genesis) = setup(root.path())?;
        let absent = WorkspaceKey {
            slot: SlotId(id(45)),
            ..workspace()
        };
        check_eq!(
            catalog.exact_history_floor(absent, 7),
            Err(IndexError::SnapshotUnavailable)
        );
        check_eq!(
            catalog.advance_exact_history_floor(absent, 7, 0, &mut admission_budget()),
            Err(IndexError::SnapshotUnavailable)
        );
        check_eq!(
            catalog.advance_exact_history_floor(workspace(), 8, 0, &mut admission_budget()),
            Err(IndexError::SnapshotUnavailable)
        );
        let other_profile = object(113, vec![]);
        let mut other = view(0, other_profile.reference, None);
        other.identity.workspace = absent;
        other.identity.epoch = 8;
        other.identity.digest = other.identity_digest()?;
        let stage = catalog.begin_bootstrap(
            other.identity.workspace,
            other.identity.epoch,
            &GenesisIdentity(other_profile.clone()),
            &mut admission_budget(),
        )?;
        catalog.reserve_bootstrap(stage, &other_profile)?;
        catalog.confirm_bootstrap(stage, &other_profile)?;
        check_eq!(
            catalog.publish_bootstrap(stage, other.clone(), &mut admission_budget())?,
            other.identity
        );
        let operation = operation(907, 114);
        let attempt = admit_new(&catalog, operation)?;
        let one = view(1, genesis.profile, None);
        check_eq!(
            publish(&catalog, attempt, &genesis, one.clone())?,
            OperationStatus::Published(one.identity)
        );
        check_eq!(
            catalog.advance_exact_history_floor(workspace(), 7, 1, &mut admission_budget())?,
            1
        );
        check_eq!(catalog.exact_history_floor(absent, 8)?, 0);
        let session = catalog.open_read_session()?;
        let other_pin = acquire_ticket_with_test_budget(
            &catalog,
            session.as_ref(),
            ViewSelection::Exact(other.identity),
            Duration::from_secs(30),
        )?;
        check_eq!(other_pin.view, other);
        catalog.release_ticket(session.as_ref(), other_pin.lease)?;
        Ok(())
    }

    #[test]
    fn exact_history_floor_budget_and_cancel_leave_zero_floor() -> TestResult {
        let root = tempfile::tempdir()?;
        let (catalog, genesis) = setup(root.path())?;
        let operation = operation(908, 115);
        let attempt = admit_new(&catalog, operation)?;
        let one = view(1, genesis.profile, None);
        check_eq!(
            publish(&catalog, attempt, &genesis, one.clone())?,
            OperationStatus::Published(one.identity)
        );
        let mut exhausted = WorkBudget::new(
            WorkLimits {
                reads: 100,
                writes: 0,
                bytes: 1 << 20,
                mutations: 100,
            },
            Instant::now() + Duration::from_secs(30),
        );
        check_eq!(
            catalog.advance_exact_history_floor(workspace(), 7, 1, &mut exhausted),
            Err(IndexError::ResourceLimited)
        );
        check_eq!(catalog.exact_history_floor(workspace(), 7)?, 0);
        let request = quanta_index_core::RequestBudgetV1::for_duration(Duration::from_secs(30));
        request.cancel_handle().cancel();
        let mut cancelled = WorkBudget::with_request(
            WorkLimits {
                reads: 100,
                writes: 100,
                bytes: 1 << 20,
                mutations: 100,
            },
            request,
        );
        check_eq!(
            catalog.advance_exact_history_floor(workspace(), 7, 1, &mut cancelled),
            Err(IndexError::Cancelled)
        );
        check_eq!(catalog.exact_history_floor(workspace(), 7)?, 0);
        check_eq!(
            catalog.advance_exact_history_floor(workspace(), 7, 1, &mut admission_budget())?,
            1
        );
        Ok(())
    }

    #[test]
    fn exact_history_floor_corrupt_digest_and_ahead_shape_fail_closed() -> TestResult {
        let root = tempfile::tempdir()?;
        let (catalog, genesis) = setup(root.path())?;
        let operation = operation(909, 116);
        let attempt = admit_new(&catalog, operation)?;
        let one = view(1, genesis.profile, None);
        check_eq!(
            publish(&catalog, attempt, &genesis, one.clone())?,
            OperationStatus::Published(one.identity)
        );
        check_eq!(
            catalog.advance_exact_history_floor(workspace(), 7, 1, &mut admission_budget())?,
            1
        );
        let workspace_blob = workspace_key(workspace())?;
        let connection = Connection::open(catalog.path())?;
        check_eq!(
            connection.execute(
                "UPDATE native_exact_history_floor_v1 SET row_digest=?1 WHERE workspace=?2 AND epoch=?3",
                params![vec![0_u8; 32], &workspace_blob, 7_i64],
            )?,
            1
        );
        check!(matches!(
            catalog.exact_history_floor(workspace(), 7),
            Err(IndexError::Storage(_))
        ));
        let session = catalog.open_read_session()?;
        check!(matches!(
            acquire_ticket_with_test_budget(
                &catalog,
                session.as_ref(),
                ViewSelection::Exact(one.identity),
                Duration::from_secs(30),
            ),
            Err(IndexError::Storage(_))
        ));
        check!(matches!(
            catalog.advance_exact_history_floor(workspace(), 7, 1, &mut admission_budget()),
            Err(IndexError::Storage(_))
        ));
        let forged_ahead = 3_i64;
        check_eq!(
            connection.execute(
                "UPDATE native_exact_history_floor_v1 SET retain_from_sequence=?1,row_digest=?2 WHERE workspace=?3 AND epoch=?4",
                params![
                    forged_ahead,
                    fields_digest(
                        b"qidx.native.exactfloor.v1",
                        &(&workspace_blob, 7_i64, forged_ahead)
                    )?,
                    &workspace_blob,
                    7_i64
                ],
            )?,
            1
        );
        check!(matches!(
            catalog.exact_history_floor(workspace(), 7),
            Err(IndexError::Storage(_))
        ));
        let stored: i64 = connection.query_row(
            "SELECT retain_from_sequence FROM native_exact_history_floor_v1 WHERE workspace=?1 AND epoch=?2",
            params![&workspace_blob, 7_i64],
            |row| row.get(0),
        )?;
        check_eq!(stored, forged_ahead);
        Ok(())
    }

    fn publish_plain_next(
        catalog: &SqliteCatalog,
        prior: &CatalogView,
        sequence: u64,
        request: u8,
    ) -> Result<CatalogView, Box<dyn Error>> {
        let operation = operation(
            920_u64
                .checked_add(sequence)
                .ok_or("test sequence overflow")?,
            request,
        );
        let attempt = admit_new(catalog, operation)?;
        let next = view(sequence, prior.profile, None);
        check_eq!(
            publish(catalog, attempt, prior, next.clone())?,
            OperationStatus::Published(next.identity)
        );
        Ok(next)
    }

    fn view_rows(catalog: &SqliteCatalog, sequence: u64) -> Result<i64, Box<dyn Error>> {
        let connection = Connection::open(catalog.path())?;
        Ok(connection.query_row(
            "SELECT COUNT(*) FROM native_view_v1 WHERE workspace=?1 AND epoch=?2 AND sequence=?3",
            params![workspace_key(workspace())?, 7_i64, int(sequence)?],
            |row| row.get(0),
        )?)
    }

    fn publish_work_next(
        catalog: &SqliteCatalog,
        prior: &CatalogView,
        sequence: u64,
        intent: &WorkIntent,
        operation_sequence: u64,
        request: u8,
    ) -> Result<CatalogView, Box<dyn Error>> {
        let attempt = admit_new(catalog, operation(operation_sequence, request))?;
        let next = view(sequence, prior.profile, None);
        check_eq!(
            commit_actions(
                catalog,
                PreparedPublication {
                    attempt,
                    expected: prior.identity,
                    next: next.clone(),
                    work_actions: WorkActionSet::empty(),
                    complete_work: None,
                    source_events: vec![],
                    projection_events: vec![],
                    expected_source: None,
                    source_mode: SourceOperationMode::Unchanged,
                },
                &[WorkAction::Create(*intent)],
            )?,
            OperationStatus::Published(next.identity)
        );
        Ok(next)
    }

    fn terminal_work_rows(
        catalog: &SqliteCatalog,
        intent: &WorkIntent,
    ) -> Result<i64, Box<dyn Error>> {
        let connection = Connection::open(catalog.path())?;
        Ok(connection.query_row(
            "SELECT COUNT(*) FROM native_work_v1 WHERE work=?1",
            params![work_key(intent)?],
            |row| row.get(0),
        )?)
    }

    fn set_terminal_work_status(
        catalog: &SqliteCatalog,
        intent: &WorkIntent,
        status: &WorkStatus,
    ) -> Result<(), Box<dyn Error>> {
        let mut guard = catalog.lock()?;
        let tx = begin(&mut guard)?;
        let mut row = read_work(&tx, intent)?.ok_or("missing work row")?;
        row.status = *status;
        write_work(&tx, &row, false)?;
        tx.commit()?;
        drop(guard);
        Ok(())
    }

    fn retirement_owned_policy() -> OwnedWorkPolicy {
        OwnedWorkPolicy {
            dispatch_limits: WorkLimits {
                reads: 100,
                writes: 100,
                bytes: 4096,
                mutations: 100,
            },
            max_dispatches: 2,
            total_timeout_ms: 600_000,
            lease_ms: 30_000,
            retry_backoff_ms: 1,
        }
    }

    fn block_managed_work(
        catalog: &SqliteCatalog,
        intent: &WorkIntent,
    ) -> Result<OwnedWorkAssignment, Box<dyn Error>> {
        let assignment = catalog
            .acquire_owned_work(intent, retirement_owned_policy(), &admission_budget())?
            .ok_or("missing managed assignment")?;
        let grant = require_owned_grant(
            catalog.begin_owned_work(&assignment)?,
            "missing managed grant",
        )?;
        insert_refused_owned_receipt(
            catalog,
            grant.assignment.request.operation,
            Refusal::ResourceLimited,
        )?;
        catalog.finish_owned_work(&grant, OwnedWorkReport::Failed(Refusal::ResourceLimited))?;
        check_eq!(
            catalog.work_status(intent, &admission_budget())?,
            WorkStatus::Blocked(Refusal::ResourceLimited)
        );
        Ok(assignment)
    }

    fn insert_refused_owned_receipt(
        catalog: &SqliteCatalog,
        key: OperationKey,
        reason: Refusal,
    ) -> Result<(), Box<dyn Error>> {
        let mut guard = catalog.lock()?;
        let tx = begin(&mut guard)?;
        write_operation(
            &tx,
            &OperationRow {
                immutable: ImmutableOperation {
                    key,
                    request: ObjectRef(digest_id(253)),
                    source_mode: SourceOperationMode::Unchanged,
                },
                events: vec![],
                fence: next_fence(&tx)?,
                status: OperationStatus::Refused(reason),
            },
            true,
        )?;
        tx.commit()?;
        drop(guard);
        Ok(())
    }

    #[test]
    fn terminal_work_retirement_explicitly_evicts_manual_status_and_aba_candidate() -> TestResult {
        let root = tempfile::tempdir()?;
        let (catalog, genesis) = setup(root.path())?;
        let intent = owner_work(141);
        let one = publish_work_next(&catalog, &genesis, 1, &intent, 941, 141)?;
        let two = publish_plain_next(&catalog, &one, 2, 142)?;
        let prepared = object(143, vec![]);
        {
            let mut guard = catalog.lock()?;
            let tx = begin(&mut guard)?;
            register(&tx, std::slice::from_ref(&prepared))?;
            protect_work_root(&tx, &intent, prepared.reference)?;
            let mut row = read_work(&tx, &intent)?.ok_or("missing work")?;
            row.prepared = Some(owner_result(&intent, prepared.reference)?);
            row.status = WorkStatus::Completed(two.identity);
            row.ownership = WorkOwnership::Manual;
            write_work(&tx, &row, false)?;
            tx.commit()?;
            drop(guard);
        }
        check_eq!(
            catalog.advance_exact_history_floor(workspace(), 7, 2, &mut admission_budget())?,
            2
        );
        let page =
            catalog.retire_terminal_work_page(workspace(), 7, None, 1, &mut admission_budget())?;
        check_eq!(
            (page.examined, page.retired, page.blocked, page.has_more),
            (1, 1, 0, false)
        );
        check_eq!(terminal_work_rows(&catalog, &intent)?, 0);
        check_eq!(
            catalog.work_status(&intent, &admission_budget())?,
            WorkStatus::Unknown
        );
        check_eq!(
            inspect_with_test_budget(&catalog, operation(941, 141).key)?,
            OperationStatus::Published(one.identity)
        );
        let connection = Connection::open(catalog.path())?;
        let object_rows: i64 = connection.query_row(
            "SELECT COUNT(*) FROM native_object_v1 WHERE object=?1",
            params![object_key(prepared.reference)],
            |row| row.get(0),
        )?;
        check_eq!(object_rows, 1);
        drop(connection);
        let views =
            catalog.retire_exact_history_page(workspace(), 7, None, 2, &mut admission_budget())?;
        check_eq!((views.retired, views.blocked), (2, 0));
        check_eq!(view_rows(&catalog, 1)?, 0);
        let three = publish_work_next(&catalog, &two, 3, &intent, 942, 144)?;
        check_eq!(
            catalog.work_status(&intent, &admission_budget())?,
            WorkStatus::Pending
        );
        let guard = catalog.lock()?;
        let fresh = read_work(&guard, &intent)?.ok_or("missing re-created work")?;
        check_eq!(fresh.created_view, three.identity);
        check_eq!(fresh.prepared, None);
        drop(guard);
        Ok(())
    }

    #[test]
    fn terminal_work_retirement_pages_skip_pending_claimed_and_managed_then_continue() -> TestResult
    {
        let root = tempfile::tempdir()?;
        let (catalog, genesis) = setup(root.path())?;
        let pending = owner_work(145);
        let claimed = owner_work(146);
        let managed = owner_work(147);
        let terminal = owner_work(148);
        let one = publish_work_next(&catalog, &genesis, 1, &pending, 943, 145)?;
        let two = publish_work_next(&catalog, &one, 2, &claimed, 944, 146)?;
        let three = publish_work_next(&catalog, &two, 3, &managed, 945, 147)?;
        let four = publish_work_next(&catalog, &three, 4, &terminal, 946, 148)?;
        let _five = publish_plain_next(&catalog, &four, 5, 149)?;
        let claim = catalog.claim_work(&claimed, Duration::from_secs(30), &admission_budget())?;
        let policy = OwnedWorkPolicy {
            dispatch_limits: WorkLimits {
                reads: 100,
                writes: 100,
                bytes: 4096,
                mutations: 100,
            },
            max_dispatches: 1,
            total_timeout_ms: 600_000,
            lease_ms: 30_000,
            retry_backoff_ms: 1,
        };
        let assignment = catalog
            .acquire_owned_work(&managed, policy, &admission_budget())?
            .ok_or("missing assignment")?;
        let _grant = require_owned_grant(catalog.begin_owned_work(&assignment)?, "missing grant")?;
        set_terminal_work_status(&catalog, &terminal, &WorkStatus::Superseded)?;
        check_eq!(
            catalog.advance_exact_history_floor(workspace(), 7, 5, &mut admission_budget())?,
            5
        );
        let first =
            catalog.retire_terminal_work_page(workspace(), 7, None, 3, &mut admission_budget())?;
        check_eq!(
            (first.examined, first.retired, first.blocked, first.has_more),
            (3, 0, 3, true)
        );
        let second = catalog.retire_terminal_work_page(
            workspace(),
            7,
            first.after,
            3,
            &mut admission_budget(),
        )?;
        check_eq!(
            (
                second.examined,
                second.retired,
                second.blocked,
                second.has_more
            ),
            (1, 1, 0, false)
        );
        for intent in [pending, claimed, managed] {
            check_eq!(terminal_work_rows(&catalog, &intent)?, 1);
        }
        check_eq!(terminal_work_rows(&catalog, &terminal)?, 0);
        check_eq!(
            catalog.work_status(&claimed, &admission_budget())?,
            WorkStatus::Claimed(claim)
        );
        check!(matches!(
            catalog.work_status(&managed, &admission_budget())?,
            WorkStatus::Claimed(_)
        ));
        drop(catalog);
        let catalog = open(root.path())?;
        let retry =
            catalog.retire_terminal_work_page(workspace(), 7, None, 8, &mut admission_budget())?;
        check_eq!(
            (retry.examined, retry.retired, retry.blocked, retry.has_more),
            (3, 0, 3, false)
        );
        Ok(())
    }

    #[test]
    fn terminal_work_retirement_budget_failure_rolls_back_entire_page() -> TestResult {
        let root = tempfile::tempdir()?;
        let (catalog, genesis) = setup(root.path())?;
        let first = owner_work(150);
        let second = owner_work(151);
        let one = publish_work_next(&catalog, &genesis, 1, &first, 947, 150)?;
        let two = publish_work_next(&catalog, &one, 2, &second, 948, 151)?;
        let _three = publish_plain_next(&catalog, &two, 3, 152)?;
        set_terminal_work_status(&catalog, &first, &WorkStatus::Superseded)?;
        set_terminal_work_status(&catalog, &second, &WorkStatus::Superseded)?;
        check_eq!(
            catalog.advance_exact_history_floor(workspace(), 7, 3, &mut admission_budget())?,
            3
        );
        let mut exhausted = WorkBudget::new(
            WorkLimits {
                reads: 100,
                writes: 1,
                bytes: 1 << 20,
                mutations: 100,
            },
            Instant::now() + Duration::from_secs(30),
        );
        check_eq!(
            catalog.retire_terminal_work_page(workspace(), 7, None, 2, &mut exhausted),
            Err(IndexError::ResourceLimited)
        );
        check_eq!(terminal_work_rows(&catalog, &first)?, 1);
        check_eq!(terminal_work_rows(&catalog, &second)?, 1);
        Ok(())
    }

    #[test]
    fn terminal_work_retirement_corrupt_lookahead_rolls_back_prior_delete() -> TestResult {
        let root = tempfile::tempdir()?;
        let (catalog, genesis) = setup(root.path())?;
        let first = owner_work(153);
        let second = owner_work(154);
        let one = publish_work_next(&catalog, &genesis, 1, &first, 949, 153)?;
        let two = publish_work_next(&catalog, &one, 2, &second, 950, 154)?;
        let _three = publish_plain_next(&catalog, &two, 3, 155)?;
        set_terminal_work_status(&catalog, &first, &WorkStatus::Superseded)?;
        set_terminal_work_status(&catalog, &second, &WorkStatus::Superseded)?;
        check_eq!(
            catalog.advance_exact_history_floor(workspace(), 7, 3, &mut admission_budget())?,
            3
        );
        let connection = Connection::open(catalog.path())?;
        check_eq!(
            connection.execute(
                "UPDATE native_work_v1 SET row_digest=?1 WHERE work=?2",
                params![vec![0_u8; 32], work_key(&second)?],
            )?,
            1
        );
        drop(connection);
        check!(matches!(
            catalog.retire_terminal_work_page(workspace(), 7, None, 1, &mut admission_budget()),
            Err(IndexError::Storage(_))
        ));
        check_eq!(terminal_work_rows(&catalog, &first)?, 1);
        check_eq!(terminal_work_rows(&catalog, &second)?, 1);
        Ok(())
    }

    #[test]
    fn terminal_work_retirement_respects_live_pin_and_floor_boundary() -> TestResult {
        let root = tempfile::tempdir()?;
        let (catalog, genesis) = setup(root.path())?;
        let session = catalog.open_read_session()?;
        let intent = owner_work(156);
        let one = publish_work_next(&catalog, &genesis, 1, &intent, 951, 156)?;
        let pin = acquire_ticket_with_test_budget(
            &catalog,
            session.as_ref(),
            ViewSelection::Exact(one.identity),
            Duration::from_secs(30),
        )?;
        let _two = publish_plain_next(&catalog, &one, 2, 157)?;
        set_terminal_work_status(&catalog, &intent, &WorkStatus::Superseded)?;
        check_eq!(
            catalog.advance_exact_history_floor(workspace(), 7, 1, &mut admission_budget())?,
            1
        );
        let none =
            catalog.retire_terminal_work_page(workspace(), 7, None, 1, &mut admission_budget())?;
        check_eq!((none.examined, none.retired, none.has_more), (0, 0, false));
        check_eq!(terminal_work_rows(&catalog, &intent)?, 1);
        check_eq!(
            catalog.advance_exact_history_floor(workspace(), 7, 2, &mut admission_budget())?,
            2
        );
        for invalid_limit in [0, 257] {
            check!(matches!(
                catalog.retire_terminal_work_page(
                    workspace(),
                    7,
                    None,
                    invalid_limit,
                    &mut admission_budget()
                ),
                Err(IndexError::InvalidInput(_))
            ));
        }
        let blocked =
            catalog.retire_terminal_work_page(workspace(), 7, None, 1, &mut admission_budget())?;
        check_eq!(
            (
                blocked.examined,
                blocked.retired,
                blocked.blocked,
                blocked.has_more
            ),
            (1, 0, 1, false)
        );
        check_eq!(terminal_work_rows(&catalog, &intent)?, 1);
        catalog.validate_ticket(session.as_ref(), pin.lease)?;
        catalog.release_ticket(session.as_ref(), pin.lease)?;
        let retired =
            catalog.retire_terminal_work_page(workspace(), 7, None, 1, &mut admission_budget())?;
        check_eq!(
            (
                retired.examined,
                retired.retired,
                retired.blocked,
                retired.has_more
            ),
            (1, 1, 0, false)
        );
        let stable = catalog.retire_terminal_work_page(
            workspace(),
            7,
            retired.after,
            1,
            &mut admission_budget(),
        )?;
        check_eq!(
            (
                stable.examined,
                stable.retired,
                stable.blocked,
                stable.has_more
            ),
            (0, 0, 0, false)
        );
        Ok(())
    }

    #[test]
    fn terminal_work_retirement_refuses_extra_child_custody_without_partial_deletion() -> TestResult
    {
        let root = tempfile::tempdir()?;
        let (catalog, genesis) = setup(root.path())?;
        let intent = owner_work(158);
        let one = publish_work_next(&catalog, &genesis, 1, &intent, 952, 158)?;
        let _two = publish_plain_next(&catalog, &one, 2, 159)?;
        set_terminal_work_status(&catalog, &intent, &WorkStatus::Superseded)?;
        check_eq!(
            catalog.advance_exact_history_floor(workspace(), 7, 2, &mut admission_budget())?,
            2
        );
        {
            let mut guard = catalog.lock()?;
            let tx = begin(&mut guard)?;
            for suffix in 0..5_u8 {
                let registered = object(
                    200_u8.checked_add(suffix).ok_or("fixture overflow")?,
                    vec![],
                );
                register(&tx, std::slice::from_ref(&registered))?;
                protect_work_root(&tx, &intent, registered.reference)?;
            }
            tx.commit()?;
            drop(guard);
        }
        check!(matches!(
            catalog.retire_terminal_work_page(workspace(), 7, None, 1, &mut admission_budget()),
            Err(IndexError::Storage(_))
        ));
        check_eq!(terminal_work_rows(&catalog, &intent)?, 1);
        let connection = Connection::open(catalog.path())?;
        let child_rows: i64 = connection.query_row(
            "SELECT COUNT(*) FROM native_work_object_v1 WHERE work=?1",
            params![work_key(&intent)?],
            |row| row.get(0),
        )?;
        check_eq!(child_rows, 5);
        Ok(())
    }

    #[test]
    fn terminal_work_retirement_refuses_forged_created_view_digest() -> TestResult {
        let root = tempfile::tempdir()?;
        let (catalog, genesis) = setup(root.path())?;
        let intent = owner_work(160);
        let one = publish_work_next(&catalog, &genesis, 1, &intent, 953, 160)?;
        let _two = publish_plain_next(&catalog, &one, 2, 161)?;
        {
            let mut guard = catalog.lock()?;
            let tx = begin(&mut guard)?;
            let mut row = read_work(&tx, &intent)?.ok_or("missing work")?;
            row.status = WorkStatus::Superseded;
            row.created_view.digest = Digest([0_u8; 32]);
            write_work(&tx, &row, false)?;
            tx.commit()?;
            drop(guard);
        }
        check_eq!(
            catalog.advance_exact_history_floor(workspace(), 7, 2, &mut admission_budget())?,
            2
        );
        check!(matches!(
            catalog.retire_terminal_work_page(workspace(), 7, None, 1, &mut admission_budget()),
            Err(IndexError::Storage(_))
        ));
        check_eq!(terminal_work_rows(&catalog, &intent)?, 1);
        Ok(())
    }

    #[test]
    fn managed_terminal_work_retirement_preserves_original_replay_and_fresh_rearm() -> TestResult {
        let root = tempfile::tempdir()?;
        let (catalog, genesis) = setup(root.path())?;
        let intent = owner_work(162);
        let one = publish_work_next(&catalog, &genesis, 1, &intent, 954, 162)?;
        let two = publish_plain_next(&catalog, &one, 2, 163)?;
        let assignment = block_managed_work(&catalog, &intent)?;
        check_eq!(
            inspect_with_test_budget(&catalog, assignment.request.operation)?,
            OperationStatus::Refused(Refusal::ResourceLimited)
        );
        check_eq!(
            catalog.validate_owned_work(&assignment.request)?,
            OperationStatus::Refused(Refusal::ResourceLimited)
        );
        check_eq!(
            catalog.validate_materialization(&assignment.request),
            Err(IndexError::WorkFenceLost)
        );
        check_eq!(
            catalog.advance_exact_history_floor(workspace(), 7, 2, &mut admission_budget())?,
            2
        );
        let retired =
            catalog.retire_terminal_work_page(workspace(), 7, None, 1, &mut admission_budget())?;
        check_eq!(
            (
                retired.examined,
                retired.retired,
                retired.blocked,
                retired.has_more
            ),
            (1, 1, 0, false)
        );
        check_eq!(
            catalog.work_status(&intent, &admission_budget())?,
            WorkStatus::Unknown
        );
        let connection = Connection::open(catalog.path())?;
        let key = work_key(&intent)?;
        let assignment_rows: i64 = connection.query_row(
            "SELECT COUNT(*) FROM native_owned_work_v1 WHERE work=?1",
            params![&key],
            |row| row.get(0),
        )?;
        let request_rows: i64 = connection.query_row(
            "SELECT COUNT(*) FROM native_owned_request_v1 WHERE work=?1",
            params![&key],
            |row| row.get(0),
        )?;
        check_eq!((assignment_rows, request_rows), (0, 1));
        drop(connection);
        check_eq!(
            catalog.validate_owned_work(&assignment.request)?,
            OperationStatus::Refused(Refusal::ResourceLimited)
        );
        check_eq!(
            catalog.validate_materialization(&assignment.request),
            Err(IndexError::WorkFenceLost)
        );
        check_eq!(
            inspect_with_test_budget(&catalog, assignment.request.operation)?,
            OperationStatus::Refused(Refusal::ResourceLimited)
        );
        let views =
            catalog.retire_exact_history_page(workspace(), 7, None, 2, &mut admission_budget())?;
        check_eq!((views.retired, views.blocked), (2, 0));
        check_eq!(view_rows(&catalog, 1)?, 0);
        let three = publish_work_next(&catalog, &two, 3, &intent, 955, 164)?;
        check_eq!(
            catalog.work_status(&intent, &admission_budget())?,
            WorkStatus::Pending
        );
        let guard = catalog.lock()?;
        let fresh = read_work(&guard, &intent)?.ok_or("missing fresh work")?;
        check_eq!(fresh.created_view, three.identity);
        check_eq!(fresh.prepared, None);
        drop(guard);
        check_eq!(
            catalog.validate_owned_work(&assignment.request)?,
            OperationStatus::Refused(Refusal::ResourceLimited)
        );
        Ok(())
    }

    #[test]
    fn managed_terminal_work_retirement_blocks_rotated_request_history() -> TestResult {
        let root = tempfile::tempdir()?;
        let clock = Arc::new(ManualClock::new(1_000));
        let (catalog, genesis) = setup_with_clock(root.path(), clock.clone())?;
        let intent = owner_work(165);
        let one = publish_work_next(&catalog, &genesis, 1, &intent, 956, 165)?;
        let _two = publish_plain_next(&catalog, &one, 2, 166)?;
        let assignment = catalog
            .acquire_owned_work(&intent, retirement_owned_policy(), &admission_budget())?
            .ok_or("missing first assignment")?;
        let grant = require_owned_grant(
            catalog.begin_owned_work(&assignment)?,
            "missing first grant",
        )?;
        catalog.finish_owned_work(&grant, OwnedWorkReport::Interrupted)?;
        clock.set(31_001);
        let rotated = catalog
            .acquire_owned_work(&intent, retirement_owned_policy(), &admission_budget())?
            .ok_or("missing rotated assignment")?;
        check_ne!(rotated.request.operation, assignment.request.operation);
        insert_refused_owned_receipt(&catalog, assignment.request.operation, Refusal::FenceLost)?;
        clock.set(31_002);
        let grant =
            require_owned_grant(catalog.begin_owned_work(&rotated)?, "missing rotated grant")?;
        insert_refused_owned_receipt(
            &catalog,
            rotated.request.operation,
            Refusal::ResourceLimited,
        )?;
        catalog.finish_owned_work(&grant, OwnedWorkReport::Failed(Refusal::ResourceLimited))?;
        let key = work_key(&intent)?;
        let connection = Connection::open(catalog.path())?;
        let request_rows: i64 = connection.query_row(
            "SELECT COUNT(*) FROM native_owned_request_v1 WHERE work=?1",
            params![&key],
            |row| row.get(0),
        )?;
        check_eq!(request_rows, 2);
        drop(connection);
        check_eq!(
            catalog.advance_exact_history_floor(workspace(), 7, 2, &mut admission_budget())?,
            2
        );
        let blocked =
            catalog.retire_terminal_work_page(workspace(), 7, None, 1, &mut admission_budget())?;
        check_eq!(
            (
                blocked.examined,
                blocked.retired,
                blocked.blocked,
                blocked.has_more
            ),
            (1, 0, 1, false)
        );
        check_eq!(terminal_work_rows(&catalog, &intent)?, 1);
        Ok(())
    }

    #[test]
    fn managed_terminal_work_retirement_corrupt_request_or_assignment_rolls_back_page() -> TestResult
    {
        for corrupt_table in ["native_owned_request_v1", "native_owned_work_v1"] {
            let root = tempfile::tempdir()?;
            let (catalog, genesis) = setup(root.path())?;
            let first = owner_work(167);
            let managed = owner_work(168);
            let one = publish_work_next(&catalog, &genesis, 1, &first, 957, 167)?;
            let two = publish_work_next(&catalog, &one, 2, &managed, 958, 168)?;
            let _three = publish_plain_next(&catalog, &two, 3, 169)?;
            set_terminal_work_status(&catalog, &first, &WorkStatus::Superseded)?;
            let assignment = block_managed_work(&catalog, &managed)?;
            check_eq!(
                catalog.advance_exact_history_floor(workspace(), 7, 3, &mut admission_budget())?,
                3
            );
            let connection = Connection::open(catalog.path())?;
            let changed = if corrupt_table == "native_owned_request_v1" {
                connection.execute(
                    "UPDATE native_owned_request_v1 SET row_digest=zeroblob(32) WHERE operation=?1",
                    params![operation_key(assignment.request.operation)?],
                )?
            } else {
                connection.execute(
                    "UPDATE native_owned_work_v1 SET row_digest=zeroblob(32) WHERE work=?1",
                    params![work_key(&managed)?],
                )?
            };
            check_eq!(changed, 1);
            drop(connection);
            check!(matches!(
                catalog.retire_terminal_work_page(workspace(), 7, None, 1, &mut admission_budget()),
                Err(IndexError::Storage(_))
            ));
            check_eq!(terminal_work_rows(&catalog, &first)?, 1);
            check_eq!(terminal_work_rows(&catalog, &managed)?, 1);
        }
        Ok(())
    }

    #[test]
    fn managed_terminal_work_retirement_budget_failure_rolls_back_both_rows() -> TestResult {
        let root = tempfile::tempdir()?;
        let (catalog, genesis) = setup(root.path())?;
        let first = owner_work(170);
        let second = owner_work(171);
        let one = publish_work_next(&catalog, &genesis, 1, &first, 959, 170)?;
        let two = publish_work_next(&catalog, &one, 2, &second, 960, 171)?;
        let _three = publish_plain_next(&catalog, &two, 3, 172)?;
        let first_assignment = block_managed_work(&catalog, &first)?;
        let second_assignment = block_managed_work(&catalog, &second)?;
        check_eq!(
            catalog.advance_exact_history_floor(workspace(), 7, 3, &mut admission_budget())?,
            3
        );
        let mut exhausted = WorkBudget::new(
            WorkLimits {
                reads: 1_000,
                writes: 2,
                bytes: 1 << 20,
                mutations: 1_000,
            },
            Instant::now() + Duration::from_secs(30),
        );
        check_eq!(
            catalog.retire_terminal_work_page(workspace(), 7, None, 2, &mut exhausted),
            Err(IndexError::ResourceLimited)
        );
        check_eq!(terminal_work_rows(&catalog, &first)?, 1);
        check_eq!(terminal_work_rows(&catalog, &second)?, 1);
        let _validated_status = catalog.validate_owned_work(&first_assignment.request)?;
        let _validated_status = catalog.validate_owned_work(&second_assignment.request)?;
        Ok(())
    }

    #[test]
    fn view_retirement_pages_skip_live_pin_and_pending_work_then_retry_after_restart() -> TestResult
    {
        let root = tempfile::tempdir()?;
        let (catalog, genesis) = setup(root.path())?;
        let session = catalog.open_read_session()?;
        let old_pin = acquire_ticket_with_test_budget(
            &catalog,
            session.as_ref(),
            ViewSelection::Exact(genesis.identity),
            Duration::from_secs(30),
        )?;
        let intent = owner_work(117);
        let operation = operation(921, 118);
        let attempt = admit_new(&catalog, operation)?;
        let one = view(1, genesis.profile, None);
        check_eq!(
            commit_actions(
                &catalog,
                PreparedPublication {
                    attempt,
                    expected: genesis.identity,
                    next: one.clone(),
                    work_actions: WorkActionSet::empty(),
                    complete_work: None,
                    source_events: vec![],
                    projection_events: vec![],
                    expected_source: None,
                    source_mode: SourceOperationMode::Unchanged,
                },
                &[WorkAction::Create(intent)],
            )?,
            OperationStatus::Published(one.identity)
        );
        let two = publish_plain_next(&catalog, &one, 2, 119)?;
        let three = publish_plain_next(&catalog, &two, 3, 120)?;
        let four = publish_plain_next(&catalog, &three, 4, 121)?;
        check_eq!(
            catalog.advance_exact_history_floor(workspace(), 7, 4, &mut admission_budget())?,
            4
        );
        for invalid_limit in [0, 257] {
            check!(matches!(
                catalog.retire_exact_history_page(
                    workspace(),
                    7,
                    None,
                    invalid_limit,
                    &mut admission_budget(),
                ),
                Err(IndexError::InvalidInput(_))
            ));
        }
        let first =
            catalog.retire_exact_history_page(workspace(), 7, None, 2, &mut admission_budget())?;
        check_eq!(
            (
                first.after,
                first.examined,
                first.retired,
                first.blocked,
                first.has_more
            ),
            (Some(1), 2, 0, 2, true)
        );
        catalog.validate_ticket(session.as_ref(), old_pin.lease)?;
        check_eq!(
            catalog.work_status(&intent, &admission_budget())?,
            WorkStatus::Pending
        );
        let second = catalog.retire_exact_history_page(
            workspace(),
            7,
            first.after,
            2,
            &mut admission_budget(),
        )?;
        check_eq!(
            (
                second.after,
                second.examined,
                second.retired,
                second.blocked,
                second.has_more
            ),
            (Some(3), 2, 2, 0, false)
        );
        for sequence in [0, 1, 4] {
            check_eq!(view_rows(&catalog, sequence)?, 1);
        }
        for sequence in [2, 3] {
            check_eq!(view_rows(&catalog, sequence)?, 0);
        }
        catalog.release_ticket(session.as_ref(), old_pin.lease)?;
        drop(session);
        drop(catalog);

        let catalog = open(root.path())?;
        let retry =
            catalog.retire_exact_history_page(workspace(), 7, None, 2, &mut admission_budget())?;
        check_eq!(
            (
                retry.after,
                retry.examined,
                retry.retired,
                retry.blocked,
                retry.has_more
            ),
            (Some(1), 2, 1, 1, false)
        );
        let empty = catalog.retire_exact_history_page(
            workspace(),
            7,
            retry.after,
            2,
            &mut admission_budget(),
        )?;
        check_eq!(
            (
                empty.after,
                empty.examined,
                empty.retired,
                empty.blocked,
                empty.has_more
            ),
            (Some(1), 0, 0, 0, false)
        );
        check_eq!(view_rows(&catalog, 0)?, 0);
        check_eq!(view_rows(&catalog, 1)?, 1);
        check_eq!(view_rows(&catalog, 4)?, 1);
        check_eq!(
            catalog.work_status(&intent, &admission_budget())?,
            WorkStatus::Pending
        );
        let connection = Connection::open(catalog.path())?;
        let profile_rows: i64 = connection.query_row(
            "SELECT COUNT(*) FROM native_object_v1 WHERE object=?1",
            params![object_key(four.profile)],
            |row| row.get(0),
        )?;
        check_eq!(profile_rows, 1);
        Ok(())
    }

    #[test]
    fn view_retirement_preserves_completed_work_created_view() -> TestResult {
        let root = tempfile::tempdir()?;
        let (catalog, genesis) = setup(root.path())?;
        let intent = owner_work(122);
        let operation = operation(921, 123);
        let attempt = admit_new(&catalog, operation)?;
        let one = view(1, genesis.profile, None);
        check_eq!(
            commit_actions(
                &catalog,
                PreparedPublication {
                    attempt,
                    expected: genesis.identity,
                    next: one.clone(),
                    work_actions: WorkActionSet::empty(),
                    complete_work: None,
                    source_events: vec![],
                    projection_events: vec![],
                    expected_source: None,
                    source_mode: SourceOperationMode::Unchanged,
                },
                &[WorkAction::Create(intent)],
            )?,
            OperationStatus::Published(one.identity)
        );
        let two = publish_plain_next(&catalog, &one, 2, 124)?;
        let prepared = object(125, vec![]);
        {
            let mut guard = catalog.lock()?;
            let tx = begin(&mut guard)?;
            register(&tx, std::slice::from_ref(&prepared))?;
            protect_work_root(&tx, &intent, prepared.reference)?;
            let mut row = read_work(&tx, &intent)?.ok_or("missing work row")?;
            row.prepared = Some(owner_result(&intent, prepared.reference)?);
            row.status = WorkStatus::Completed(two.identity);
            write_work(&tx, &row, false)?;
            tx.commit()?;
            drop(guard);
        }
        check_eq!(
            catalog.work_status(&intent, &admission_budget())?,
            WorkStatus::Completed(two.identity)
        );
        check_eq!(
            catalog.advance_exact_history_floor(workspace(), 7, 2, &mut admission_budget())?,
            2
        );
        let page =
            catalog.retire_exact_history_page(workspace(), 7, None, 2, &mut admission_budget())?;
        check_eq!(
            (page.examined, page.retired, page.blocked, page.has_more),
            (2, 1, 1, false)
        );
        check_eq!(view_rows(&catalog, 0)?, 0);
        check_eq!(view_rows(&catalog, 1)?, 1);
        check_eq!(view_rows(&catalog, 2)?, 1);
        check_eq!(
            catalog.work_status(&intent, &admission_budget())?,
            WorkStatus::Completed(two.identity)
        );
        Ok(())
    }

    #[test]
    fn view_retirement_budget_failure_rolls_back_entire_page() -> TestResult {
        let root = tempfile::tempdir()?;
        let (catalog, genesis) = setup(root.path())?;
        let one = publish_plain_next(&catalog, &genesis, 1, 126)?;
        let _two = publish_plain_next(&catalog, &one, 2, 127)?;
        check_eq!(
            catalog.advance_exact_history_floor(workspace(), 7, 2, &mut admission_budget())?,
            2
        );
        let mut exhausted = WorkBudget::new(
            WorkLimits {
                reads: 100,
                writes: 2,
                bytes: 1 << 20,
                mutations: 100,
            },
            Instant::now() + Duration::from_secs(30),
        );
        check_eq!(
            catalog.retire_exact_history_page(workspace(), 7, None, 2, &mut exhausted),
            Err(IndexError::ResourceLimited)
        );
        check_eq!(view_rows(&catalog, 0)?, 1);
        check_eq!(view_rows(&catalog, 1)?, 1);
        Ok(())
    }

    #[test]
    fn view_retirement_corrupt_lookahead_rolls_back_prior_delete() -> TestResult {
        let root = tempfile::tempdir()?;
        let (catalog, genesis) = setup(root.path())?;
        let one = publish_plain_next(&catalog, &genesis, 1, 128)?;
        let _two = publish_plain_next(&catalog, &one, 2, 129)?;
        check_eq!(
            catalog.advance_exact_history_floor(workspace(), 7, 2, &mut admission_budget())?,
            2
        );
        let connection = Connection::open(catalog.path())?;
        check_eq!(
            connection.execute(
                "UPDATE native_view_v1 SET row_digest=?1 WHERE workspace=?2 AND epoch=?3 AND sequence=?4",
                params![vec![0_u8; 32], workspace_key(workspace())?, 7_i64, 1_i64],
            )?,
            1
        );
        drop(connection);
        check!(matches!(
            catalog.retire_exact_history_page(workspace(), 7, None, 1, &mut admission_budget()),
            Err(IndexError::Storage(_))
        ));
        check_eq!(view_rows(&catalog, 0)?, 1);
        check_eq!(view_rows(&catalog, 1)?, 1);
        Ok(())
    }

    #[test]
    fn view_retirement_live_expired_pin_still_blocks_until_explicit_release() -> TestResult {
        let root = tempfile::tempdir()?;
        let clock = Arc::new(ManualClock::new(1_000));
        let (catalog, genesis) = setup_with_clock(root.path(), clock.clone())?;
        let session = catalog.open_read_session()?;
        let old_pin = acquire_ticket_with_test_budget(
            &catalog,
            session.as_ref(),
            ViewSelection::Exact(genesis.identity),
            Duration::from_millis(10),
        )?;
        let one = publish_plain_next(&catalog, &genesis, 1, 130)?;
        check_eq!(
            catalog.advance_exact_history_floor(workspace(), 7, 1, &mut admission_budget())?,
            1
        );
        clock.set(1_011);
        check_eq!(
            catalog.validate_ticket(session.as_ref(), old_pin.lease),
            Err(IndexError::SnapshotUnavailable)
        );
        check!(matches!(
            acquire_ticket_with_test_budget(
                &catalog,
                session.as_ref(),
                ViewSelection::Exact(genesis.identity),
                Duration::from_millis(10),
            ),
            Err(IndexError::SnapshotUnavailable)
        ));
        let blocked =
            catalog.retire_exact_history_page(workspace(), 7, None, 1, &mut admission_budget())?;
        check_eq!(
            (
                blocked.after,
                blocked.examined,
                blocked.retired,
                blocked.blocked,
                blocked.has_more,
            ),
            (Some(0), 1, 0, 1, false)
        );
        check_eq!(view_rows(&catalog, 0)?, 1);
        let ahead = catalog.retire_exact_history_page(
            workspace(),
            7,
            Some(1),
            1,
            &mut admission_budget(),
        )?;
        check_eq!(
            (
                ahead.after,
                ahead.examined,
                ahead.retired,
                ahead.blocked,
                ahead.has_more
            ),
            (Some(1), 0, 0, 0, false)
        );
        catalog.release_ticket(session.as_ref(), old_pin.lease)?;
        let retired =
            catalog.retire_exact_history_page(workspace(), 7, None, 1, &mut admission_budget())?;
        check_eq!(
            (retired.examined, retired.retired, retired.blocked),
            (1, 1, 0)
        );
        check_eq!(view_rows(&catalog, 0)?, 0);
        check_eq!(view_rows(&catalog, 1)?, 1);
        let latest = acquire_ticket_with_test_budget(
            &catalog,
            session.as_ref(),
            ViewSelection::Latest {
                workspace: workspace(),
                epoch: 7,
            },
            Duration::from_millis(10),
        )?;
        check_eq!(latest.view, one);
        catalog.release_ticket(session.as_ref(), latest.lease)?;
        Ok(())
    }

    fn publish_retired_work_next(
        catalog: &SqliteCatalog,
        prior: &CatalogView,
        sequence: u64,
        intent: &WorkIntent,
        operation_sequence: u64,
        request: u8,
    ) -> Result<CatalogView, Box<dyn Error>> {
        let attempt = admit_new(catalog, operation(operation_sequence, request))?;
        let next = view(sequence, prior.profile, None);
        check_eq!(
            commit_actions(
                catalog,
                PreparedPublication {
                    attempt,
                    expected: prior.identity,
                    next: next.clone(),
                    work_actions: WorkActionSet::empty(),
                    complete_work: None,
                    source_events: vec![],
                    projection_events: vec![],
                    expected_source: None,
                    source_mode: SourceOperationMode::Unchanged,
                },
                &[WorkAction::Retire(*intent)],
            )?,
            OperationStatus::Published(next.identity)
        );
        Ok(next)
    }

    #[test]
    fn never_admitted_managed_assignment_retires_without_fabricating_replay() -> TestResult {
        let root = tempfile::tempdir()?;
        let (catalog, genesis) = setup(root.path())?;
        let intent = owner_work(176);
        let one = publish_work_next(&catalog, &genesis, 1, &intent, 970, 176)?;
        let assignment = catalog
            .acquire_owned_work(&intent, retirement_owned_policy(), &admission_budget())?
            .ok_or("missing never-dispatched assignment")?;
        check_eq!(assignment.dispatches, 0);
        check!(!assignment.active);
        check_eq!(
            inspect_with_test_budget(&catalog, assignment.request.operation)?,
            OperationStatus::Unknown
        );
        let two = publish_retired_work_next(&catalog, &one, 2, &intent, 971, 177)?;
        check_eq!(
            catalog.work_status(&intent, &admission_budget())?,
            WorkStatus::Superseded
        );
        check!(matches!(
            catalog.validate_owned_work(&assignment.request),
            Err(IndexError::WorkFenceLost)
        ));
        let session = catalog.open_read_session()?;
        let pin = acquire_ticket_with_test_budget(
            &catalog,
            session.as_ref(),
            ViewSelection::Exact(one.identity),
            Duration::from_secs(30),
        )?;
        check_eq!(
            catalog.advance_exact_history_floor(workspace(), 7, 2, &mut admission_budget())?,
            2
        );
        let blocked =
            catalog.retire_terminal_work_page(workspace(), 7, None, 1, &mut admission_budget())?;
        check_eq!(
            (blocked.examined, blocked.retired, blocked.blocked),
            (1, 0, 1)
        );
        catalog.release_ticket(session.as_ref(), pin.lease)?;
        let retired =
            catalog.retire_terminal_work_page(workspace(), 7, None, 1, &mut admission_budget())?;
        check_eq!(
            (retired.examined, retired.retired, retired.blocked),
            (1, 1, 0)
        );
        check_eq!(
            catalog.work_status(&intent, &admission_budget())?,
            WorkStatus::Unknown
        );
        check_eq!(
            inspect_with_test_budget(&catalog, assignment.request.operation)?,
            OperationStatus::Unknown
        );
        check!(matches!(
            catalog.validate_owned_work(&assignment.request),
            Err(IndexError::WorkFenceLost)
        ));
        let connection = Connection::open(catalog.path())?;
        let key = work_key(&intent)?;
        for table in [
            "native_work_v1",
            "native_work_object_v1",
            "native_owned_work_v1",
            "native_owned_request_v1",
        ] {
            let query = format!("SELECT COUNT(*) FROM {table} WHERE work=?1");
            let rows: i64 = connection.query_row(&query, params![&key], |row| row.get(0))?;
            check_eq!(rows, 0);
        }
        drop(connection);
        let three = publish_work_next(&catalog, &two, 3, &intent, 972, 178)?;
        check_eq!(
            catalog.work_status(&intent, &admission_budget())?,
            WorkStatus::Pending
        );
        let fresh = catalog
            .acquire_owned_work(&intent, retirement_owned_policy(), &admission_budget())?
            .ok_or("fresh work did not rearm")?;
        check_ne!(fresh.request.operation, assignment.request.operation);
        check_eq!(fresh.request.expected_view, three.identity);
        check!(matches!(
            catalog.validate_owned_work(&assignment.request),
            Err(IndexError::WorkFenceLost)
        ));
        Ok(())
    }

    #[test]
    fn started_managed_assignment_without_receipt_remains_protected() -> TestResult {
        let root = tempfile::tempdir()?;
        let (catalog, genesis) = setup(root.path())?;
        let intent = owner_work(179);
        let one = publish_work_next(&catalog, &genesis, 1, &intent, 973, 179)?;
        let assignment = catalog
            .acquire_owned_work(&intent, retirement_owned_policy(), &admission_budget())?
            .ok_or("missing managed assignment")?;
        let grant = require_owned_grant(
            catalog.begin_owned_work(&assignment)?,
            "missing begun dispatch",
        )?;
        check_eq!(grant.ordinal, 1);
        let _two = publish_retired_work_next(&catalog, &one, 2, &intent, 974, 180)?;
        check_eq!(
            inspect_with_test_budget(&catalog, assignment.request.operation)?,
            OperationStatus::Unknown
        );
        check_eq!(
            catalog.advance_exact_history_floor(workspace(), 7, 2, &mut admission_budget())?,
            2
        );
        let blocked =
            catalog.retire_terminal_work_page(workspace(), 7, None, 1, &mut admission_budget())?;
        check_eq!(
            (blocked.examined, blocked.retired, blocked.blocked),
            (1, 0, 1)
        );
        check_eq!(terminal_work_rows(&catalog, &intent)?, 1);
        Ok(())
    }

    #[test]
    fn never_admitted_managed_corruption_rolls_back_prior_retirement() -> TestResult {
        for corrupt_table in ["native_owned_request_v1", "native_owned_work_v1"] {
            let root = tempfile::tempdir()?;
            let (catalog, genesis) = setup(root.path())?;
            let first = owner_work(181);
            let managed = owner_work(182);
            let one = publish_work_next(&catalog, &genesis, 1, &first, 975, 181)?;
            let two = publish_work_next(&catalog, &one, 2, &managed, 976, 182)?;
            set_terminal_work_status(&catalog, &first, &WorkStatus::Superseded)?;
            let assignment = catalog
                .acquire_owned_work(&managed, retirement_owned_policy(), &admission_budget())?
                .ok_or("missing never-dispatched assignment")?;
            check_eq!(assignment.dispatches, 0);
            let _three = publish_retired_work_next(&catalog, &two, 3, &managed, 977, 183)?;
            check_eq!(
                catalog.advance_exact_history_floor(workspace(), 7, 3, &mut admission_budget())?,
                3
            );
            let connection = Connection::open(catalog.path())?;
            let changed = if corrupt_table == "native_owned_request_v1" {
                connection.execute(
                    "UPDATE native_owned_request_v1 SET row_digest=zeroblob(32) WHERE operation=?1",
                    params![operation_key(assignment.request.operation)?],
                )?
            } else {
                connection.execute(
                    "UPDATE native_owned_work_v1 SET row_digest=zeroblob(32) WHERE work=?1",
                    params![work_key(&managed)?],
                )?
            };
            check_eq!(changed, 1);
            drop(connection);
            check!(matches!(
                catalog.retire_terminal_work_page(workspace(), 7, None, 1, &mut admission_budget()),
                Err(IndexError::Storage(_))
            ));
            check_eq!(terminal_work_rows(&catalog, &first)?, 1);
            check_eq!(terminal_work_rows(&catalog, &managed)?, 1);
        }
        Ok(())
    }

    fn provisional_child_rows(
        catalog: &SqliteCatalog,
        parent: ObjectRef,
        child: ObjectRef,
    ) -> Result<(i64, i64, i64), Box<dyn Error>> {
        let connection = Connection::open(catalog.path())?;
        let installation: i64 = connection.query_row(
            "SELECT COUNT(*) FROM native_object_install_v1 WHERE object=?1",
            params![object_key(parent)],
            |row| row.get(0),
        )?;
        let provisional: i64 = connection.query_row(
            "SELECT COUNT(*) FROM native_object_install_edge_v1 WHERE parent=?1 AND child=?2",
            params![object_key(parent), object_key(child)],
            |row| row.get(0),
        )?;
        let permanent: i64 = connection.query_row(
            "SELECT COUNT(*) FROM native_object_edge_v1 WHERE parent=?1 AND child=?2",
            params![object_key(parent), object_key(child)],
            |row| row.get(0),
        )?;
        Ok((installation, provisional, permanent))
    }

    #[test]
    fn unconfirmed_attempt_parent_keeps_child_custody_after_terminal_release_and_reopen()
    -> TestResult {
        let root = tempfile::tempdir()?;
        let (catalog, genesis) = setup(root.path())?;
        let child = genesis.profile;
        let parent = object(211, vec![child]);
        let first = operation(1001, 212);
        let attempt = admit_new(&catalog, first)?;
        catalog.reserve_objects(attempt, std::slice::from_ref(&parent))?;
        check_eq!(
            provisional_child_rows(&catalog, parent.reference, child)?,
            (1, 1, 0)
        );
        check_eq!(
            catalog.refuse(attempt, Refusal::InvalidInput)?,
            OperationStatus::Refused(Refusal::InvalidInput)
        );
        let release = catalog.release_terminal_custody(first.key, 256, &mut admission_budget())?;
        check!(!release.has_more);
        check_eq!(terminal_custody_counts(&catalog, first.key)?, (0, 0, 0));
        drop(catalog);

        let catalog = open(root.path())?;
        check_eq!(
            provisional_child_rows(&catalog, parent.reference, child)?,
            (1, 1, 0)
        );
        let second = operation(1002, 213);
        let second_attempt = admit_new(&catalog, second)?;
        catalog.reserve_objects(second_attempt, std::slice::from_ref(&parent))?;
        catalog.register_objects(second_attempt, std::slice::from_ref(&parent))?;
        check_eq!(
            provisional_child_rows(&catalog, parent.reference, child)?,
            (0, 0, 1)
        );
        let third = operation(1005, 217);
        let third_attempt = admit_new(&catalog, third)?;
        catalog.reserve_objects(third_attempt, std::slice::from_ref(&parent))?;
        check_eq!(
            provisional_child_rows(&catalog, parent.reference, child)?,
            (0, 0, 1)
        );
        catalog.register_objects(third_attempt, std::slice::from_ref(&parent))?;
        check_eq!(
            provisional_child_rows(&catalog, parent.reference, child)?,
            (0, 0, 1)
        );
        check_eq!(
            inspect_with_test_budget(&catalog, first.key)?,
            OperationStatus::Refused(Refusal::InvalidInput)
        );
        Ok(())
    }

    #[test]
    fn discarded_ingress_keeps_unconfirmed_child_custody_and_rejects_changed_metadata() -> TestResult
    {
        let root = tempfile::tempdir()?;
        let (catalog, genesis) = setup(root.path())?;
        let child = genesis.profile;
        let parent = object(214, vec![child]);
        let first = operation(1003, 215);
        let ingress = catalog.open_ingress(first.key)?;
        catalog.reserve_inputs(ingress, std::slice::from_ref(&parent))?;
        catalog.discard_inputs(ingress, &mut admission_budget())?;
        check_eq!(fixture_read_ingress(&catalog, first.key)?, None);
        drop(catalog);

        let catalog = open(root.path())?;
        check_eq!(
            provisional_child_rows(&catalog, parent.reference, child)?,
            (1, 1, 0)
        );
        let second = operation(1004, 216);
        let ingress = catalog.open_ingress(second.key)?;
        let changed = RegisteredObject::unscoped(parent.reference, 2, parent.children.clone());
        check!(matches!(
            catalog.reserve_inputs(ingress, std::slice::from_ref(&changed)),
            Err(IndexError::CorruptObject(_))
        ));
        check_eq!(
            provisional_child_rows(&catalog, parent.reference, child)?,
            (1, 1, 0)
        );
        Ok(())
    }

    #[test]
    fn later_reservation_failure_rolls_back_earlier_provisional_registration() -> TestResult {
        let root = tempfile::tempdir()?;
        let (catalog, genesis) = setup(root.path())?;
        let child = genesis.profile;
        let prior = object(218, vec![child]);
        let first = catalog.open_ingress(operation(1006, 219).key)?;
        catalog.reserve_inputs(first, std::slice::from_ref(&prior))?;

        let fresh = object(220, vec![child]);
        let changed = RegisteredObject::unscoped(prior.reference, 2, prior.children.clone());
        let second_operation = operation(1007, 221);
        let second = catalog.open_ingress(second_operation.key)?;
        check!(matches!(
            catalog.reserve_inputs(second, &[fresh.clone(), changed]),
            Err(IndexError::CorruptObject(_))
        ));
        check_eq!(
            provisional_child_rows(&catalog, fresh.reference, child)?,
            (0, 0, 0)
        );
        check_eq!(
            provisional_child_rows(&catalog, prior.reference, child)?,
            (1, 1, 0)
        );
        let connection = Connection::open(catalog.path())?;
        let intent_count: i64 = connection.query_row(
            "SELECT COUNT(*) FROM native_ingress_intent_v1 WHERE operation=?1",
            params![operation_key(second_operation.key)?],
            |row| row.get(0),
        )?;
        check_eq!(intent_count, 0);
        Ok(())
    }

    #[test]
    fn later_confirmation_failure_rolls_back_earlier_registration_and_transfer() -> TestResult {
        let root = tempfile::tempdir()?;
        let (catalog, genesis) = setup(root.path())?;
        let attempt = admit_new(&catalog, operation(1008, 222))?;
        let middle = object(223, vec![genesis.profile]);
        reserve_confirm(&catalog, attempt, std::slice::from_ref(&middle))?;

        let first = object(224, vec![genesis.profile]);
        let later = object(225, vec![middle.reference]);
        catalog.reserve_objects(attempt, &[first.clone(), later.clone()])?;
        let connection = Connection::open(catalog.path())?;
        let changed = connection.execute(
            "UPDATE native_object_edge_v1 SET row_digest=zeroblob(32) WHERE parent=?1",
            params![object_key(middle.reference)],
        )?;
        check_eq!(changed, 1);
        drop(connection);

        check!(matches!(
            catalog.register_objects(attempt, &[first.clone(), later.clone()]),
            Err(IndexError::CorruptObject(_))
        ));
        check_eq!(
            provisional_child_rows(&catalog, first.reference, genesis.profile)?,
            (1, 1, 0)
        );
        check_eq!(
            provisional_child_rows(&catalog, later.reference, middle.reference)?,
            (1, 1, 0)
        );
        let connection = Connection::open(catalog.path())?;
        let registered: i64 = connection.query_row(
            "SELECT COUNT(*) FROM native_object_v1 WHERE object=?1",
            params![object_key(first.reference)],
            |row| row.get(0),
        )?;
        check_eq!(registered, 0);
        Ok(())
    }

    #[test]
    fn registered_metadata_match_still_checks_bounds_and_edge_ledger() -> TestResult {
        enum Fault {
            EdgeDigest,
            MissingEdge,
            ZeroBytes,
        }

        for fault in [Fault::EdgeDigest, Fault::MissingEdge, Fault::ZeroBytes] {
            let root = tempfile::tempdir()?;
            let (catalog, genesis) = setup(root.path())?;
            let parent = object(250, vec![genesis.profile, genesis.profile]);
            let attempt = admit_new(&catalog, operation(12100, 185))?;
            reserve_confirm(&catalog, attempt, std::slice::from_ref(&parent))?;
            let guard = catalog.lock().map_err(|error| error.to_string())?;
            check!(object_install::registered_matches(&guard, &parent)?);
            drop(guard);

            let connection = Connection::open(catalog.path())?;
            let expected = match fault {
                Fault::EdgeDigest => {
                    check_eq!(
                        connection.execute(
                            "UPDATE native_object_edge_v1 SET row_digest=zeroblob(32) WHERE parent=?1",
                            params![object_key(parent.reference)],
                        )?,
                        1
                    );
                    parent.clone()
                }
                Fault::MissingEdge => {
                    check_eq!(
                        connection.execute(
                            "DELETE FROM native_object_edge_v1 WHERE parent=?1",
                            params![object_key(parent.reference)],
                        )?,
                        1
                    );
                    parent.clone()
                }
                Fault::ZeroBytes => {
                    // The row and expected metadata agree, including a valid
                    // checksum; canonical bounds still have to reject it.
                    let invalid =
                        RegisteredObject::unscoped(parent.reference, 0, parent.children.clone());
                    let payload = encode(&invalid)?;
                    check_eq!(
                        connection.execute(
                            "UPDATE native_object_v1 SET payload=?1,row_digest=?2 WHERE object=?3",
                            params![
                                &payload,
                                digest(b"qidx.native.object.v1", &payload).to_vec(),
                                object_key(parent.reference)
                            ],
                        )?,
                        1
                    );
                    invalid
                }
            };
            drop(connection);
            let guard = catalog.lock().map_err(|error| error.to_string())?;
            check!(matches!(
                object_install::registered_matches(&guard, &expected),
                Err(IndexError::CorruptObject(_))
            ));
            drop(guard);
        }
        Ok(())
    }

    #[test]
    fn corrupt_registered_child_edge_shape_refuses_new_reservation() -> TestResult {
        for overflow in [false, true] {
            let root = tempfile::tempdir()?;
            let (catalog, genesis) = setup(root.path())?;
            let connection = Connection::open(catalog.path())?;
            connection.execute_batch("PRAGMA foreign_keys=OFF; BEGIN IMMEDIATE")?;
            let mut insert = connection.prepare(
                "INSERT INTO native_object_edge_v1(parent,child,row_digest) VALUES (?1,?2,?3)",
            )?;
            if overflow {
                for slot in 0_u16..=4096_u16 {
                    let mut malformed_child = Vec::with_capacity(32);
                    malformed_child.extend_from_slice(&slot.to_be_bytes());
                    malformed_child.resize(32, 0);
                    let inserted = insert.execute(params![
                        object_key(genesis.profile),
                        malformed_child,
                        vec![0_u8; 32]
                    ])?;
                    check_eq!(inserted, 1);
                }
            } else {
                let inserted = insert.execute(params![
                    object_key(genesis.profile),
                    vec![7_u8; 31],
                    vec![0_u8; 32]
                ])?;
                check_eq!(inserted, 1);
            }
            drop(insert);
            connection.execute_batch("COMMIT")?;
            drop(connection);

            let parent = object(226, vec![genesis.profile]);
            let attempt = admit_new(&catalog, operation(1009, 227))?;
            check!(matches!(
                catalog.reserve_objects(attempt, std::slice::from_ref(&parent)),
                Err(IndexError::CorruptObject(_))
            ));
            check_eq!(
                provisional_child_rows(&catalog, parent.reference, genesis.profile)?,
                (0, 0, 0)
            );
        }
        Ok(())
    }

    #[test]
    fn oversized_reservations_refuse_before_missing_child_lookup() -> TestResult {
        for ingress in [false, true] {
            for oversized_children in [false, true] {
                let root = tempfile::tempdir()?;
                let (catalog, _genesis) = setup(root.path())?;
                let missing_child = ObjectRef(digest_id(228));
                let mut candidate = object(229, vec![missing_child]);
                if oversized_children {
                    candidate = RegisteredObject::unscoped(
                        candidate.reference,
                        candidate.bytes,
                        vec![missing_child; 4097],
                    );
                } else {
                    // The store's maximum wire is 16-byte header + 4096 refs + 16 MiB.
                    candidate = RegisteredObject::unscoped(
                        candidate.reference,
                        16 * 1024 * 1024 + 4096 * 32 + 17,
                        candidate.children.clone(),
                    );
                }
                let request = operation(1010, 230);
                let result = if ingress {
                    let stage = catalog.open_ingress(request.key)?;
                    catalog.reserve_inputs(stage, std::slice::from_ref(&candidate))
                } else {
                    let attempt = admit_new(&catalog, request)?;
                    catalog.reserve_objects(attempt, std::slice::from_ref(&candidate))
                };
                check!(matches!(result, Err(IndexError::ResourceLimited)));
                check_eq!(
                    provisional_child_rows(&catalog, candidate.reference, missing_child)?,
                    (0, 0, 0)
                );
                let connection = Connection::open(catalog.path())?;
                let operation = operation_key(request.key)?;
                let attempt_intents: i64 = connection.query_row(
                    "SELECT COUNT(*) FROM native_object_intent_v1 WHERE operation=?1",
                    params![&operation],
                    |row| row.get(0),
                )?;
                let ingress_intents: i64 = connection.query_row(
                    "SELECT COUNT(*) FROM native_ingress_intent_v1 WHERE operation=?1",
                    params![&operation],
                    |row| row.get(0),
                )?;
                check_eq!((attempt_intents, ingress_intents), (0, 0));
            }
        }
        Ok(())
    }

    struct GenesisIdentity(RegisteredObject);

    impl ImmutableObjectPort for GenesisIdentity {
        fn identify(
            &self,
            payload: &[u8],
            children: &[ObjectRef],
            budget: &mut WorkBudget,
        ) -> Result<RegisteredObject, IndexError> {
            budget.checkpoint()?;
            let expected = encode(&quanta_index_core::index::IndexProfile {
                instances: Vec::new(),
            })?;
            if payload != expected || !children.is_empty() {
                return Err(IndexError::InvalidInput(
                    "genesis identity input differs from empty profile".into(),
                ));
            }
            Ok(self.0.clone())
        }

        fn put(
            &self,
            _payload: &[u8],
            _children: &[ObjectRef],
            _budget: &mut WorkBudget,
        ) -> Result<RegisteredObject, IndexError> {
            Err(IndexError::BackendViolation("identity-only fixture".into()))
        }

        fn install(
            &self,
            _payload: &[u8],
            _children: &[ObjectRef],
            _sink: &dyn quanta_index_core::index::ports::ObjectWriteSink,
            _budget: &mut WorkBudget,
        ) -> Result<RegisteredObject, IndexError> {
            Err(IndexError::BackendViolation("identity-only fixture".into()))
        }

        fn install_batch(
            &self,
            _inputs: &[quanta_index_core::index::ObjectBatchInput],
            _sink: &dyn quanta_index_core::index::ports::ObjectWriteSink,
            _budget: &mut WorkBudget,
        ) -> Result<quanta_index_core::index::RegisteredObjectBatch, IndexError> {
            Err(IndexError::BackendViolation("identity-only fixture".into()))
        }

        fn get(
            &self,
            _reference: ObjectRef,
            _budget: &mut WorkBudget,
        ) -> Result<quanta_index_core::index::StoredObject, IndexError> {
            Err(IndexError::BackendViolation("identity-only fixture".into()))
        }
    }

    #[test]
    fn bootstrap_stage_reopens_with_exact_frozen_profile_and_epoch() -> TestResult {
        let root = tempfile::tempdir()?;
        let catalog = open(root.path())?;
        let profile = object(231, vec![]);
        let stage = catalog.begin_bootstrap(
            workspace(),
            7,
            &GenesisIdentity(profile.clone()),
            &mut admission_budget(),
        )?;
        check_eq!(stage.workspace, workspace());
        check_eq!(stage.epoch, 7);
        check_eq!(stage.profile, profile.reference);
        check!(stage.fence > 0);
        drop(catalog);

        let catalog = open(root.path())?;
        check_eq!(
            catalog.begin_bootstrap(
                workspace(),
                7,
                &GenesisIdentity(profile.clone()),
                &mut admission_budget(),
            )?,
            stage
        );
        check_eq!(
            catalog.begin_bootstrap(
                workspace(),
                8,
                &GenesisIdentity(profile.clone()),
                &mut admission_budget(),
            ),
            Err(IndexError::Conflict)
        );
        check_eq!(
            catalog.begin_bootstrap(
                workspace(),
                7,
                &GenesisIdentity(object(232, vec![])),
                &mut admission_budget(),
            ),
            Err(IndexError::Conflict)
        );
        let changed_metadata =
            RegisteredObject::unscoped((profile).reference, 2, (profile).children.clone());
        check!(matches!(
            catalog.begin_bootstrap(
                workspace(),
                7,
                &GenesisIdentity(changed_metadata),
                &mut admission_budget(),
            ),
            Err(IndexError::CorruptObject(_))
        ));
        Ok(())
    }

    #[test]
    fn bootstrap_confirmation_and_genesis_publication_transfer_root_custody_atomically()
    -> TestResult {
        let root = tempfile::tempdir()?;
        let catalog = open(root.path())?;
        let profile = object(233, vec![]);
        let genesis = view(0, profile.reference, None);
        let stage = catalog.begin_bootstrap(
            workspace(),
            7,
            &GenesisIdentity(profile.clone()),
            &mut admission_budget(),
        )?;
        check!(matches!(
            catalog.publish_bootstrap(stage, genesis.clone(), &mut admission_budget()),
            Err(IndexError::BackendViolation(_))
        ));
        let connection = Connection::open(catalog.path())?;
        let heads: i64 = connection.query_row(
            "SELECT COUNT(*) FROM native_workspace_v1 WHERE workspace=?1",
            params![workspace_key(workspace())?],
            |row| row.get(0),
        )?;
        check_eq!(heads, 0);
        drop(connection);

        catalog.reserve_bootstrap(stage, &profile)?;
        check_eq!(
            provisional_child_rows(&catalog, profile.reference, profile.reference)?.0,
            1
        );
        drop(catalog);
        let catalog = open(root.path())?;
        check_eq!(
            catalog.begin_bootstrap(
                workspace(),
                7,
                &GenesisIdentity(profile.clone()),
                &mut admission_budget(),
            )?,
            stage
        );
        catalog.confirm_bootstrap(stage, &profile)?;
        check_eq!(
            provisional_child_rows(&catalog, profile.reference, profile.reference)?.0,
            0
        );
        drop(catalog);

        let catalog = open(root.path())?;
        check_eq!(
            catalog.publish_bootstrap(stage, genesis.clone(), &mut admission_budget())?,
            genesis.identity
        );
        let connection = Connection::open(catalog.path())?;
        let stages: i64 = connection.query_row(
            "SELECT COUNT(*) FROM native_bootstrap_v1 WHERE workspace=?1",
            params![workspace_key(workspace())?],
            |row| row.get(0),
        )?;
        let roots: i64 = connection.query_row(
            "SELECT COUNT(*) FROM native_view_root_v1 WHERE workspace=?1 AND epoch=7 AND sequence=0 AND object=?2",
            params![workspace_key(workspace())?, object_key(profile.reference)],
            |row| row.get(0),
        )?;
        check_eq!((stages, roots), (0, 1));
        check_eq!(
            catalog.begin_bootstrap(
                workspace(),
                7,
                &GenesisIdentity(profile),
                &mut admission_budget(),
            ),
            Err(IndexError::Conflict)
        );
        Ok(())
    }

    #[test]
    fn forged_bootstrap_reference_and_wrong_root_cannot_publish() -> TestResult {
        let root = tempfile::tempdir()?;
        let catalog = open(root.path())?;
        let profile = object(234, vec![]);
        let stage = catalog.begin_bootstrap(
            workspace(),
            7,
            &GenesisIdentity(profile.clone()),
            &mut admission_budget(),
        )?;
        let forged = BootstrapRef {
            fence: stage.fence.checked_add(1).ok_or("fence overflow")?,
            ..stage
        };
        check_eq!(
            catalog.reserve_bootstrap(forged, &profile),
            Err(IndexError::FenceLost)
        );
        let forged_workspace = BootstrapRef {
            workspace: WorkspaceKey {
                slot: SlotId(id(238)),
                ..workspace()
            },
            ..stage
        };
        let forged_epoch = BootstrapRef { epoch: 8, ..stage };
        check_eq!(
            catalog.reserve_bootstrap(forged_workspace, &profile),
            Err(IndexError::FenceLost)
        );
        check_eq!(
            catalog.reserve_bootstrap(forged_epoch, &profile),
            Err(IndexError::FenceLost)
        );
        let changed_bytes =
            RegisteredObject::unscoped(profile.reference, 2, profile.children.clone());
        check!(matches!(
            catalog.reserve_bootstrap(stage, &changed_bytes),
            Err(IndexError::CorruptObject(_))
        ));
        let changed_children = RegisteredObject::unscoped(
            profile.reference,
            profile.bytes,
            vec![object(239, vec![]).reference],
        );
        check!(matches!(
            catalog.reserve_bootstrap(stage, &changed_children),
            Err(IndexError::BackendViolation(_))
        ));
        let connection = Connection::open(catalog.path())?;
        let intents: i64 = connection.query_row(
            "SELECT COUNT(*) FROM native_bootstrap_object_v1 WHERE workspace=?1",
            params![workspace_key(workspace())?],
            |row| row.get(0),
        )?;
        check_eq!(intents, 0);
        drop(connection);
        catalog.reserve_bootstrap(stage, &profile)?;
        check_eq!(
            catalog.confirm_bootstrap(forged, &profile),
            Err(IndexError::FenceLost)
        );
        check!(matches!(
            catalog.confirm_bootstrap(stage, &changed_bytes),
            Err(IndexError::CorruptObject(_))
        ));
        let connection = Connection::open(catalog.path())?;
        let confirmed: i64 = connection.query_row(
            "SELECT confirmed FROM native_bootstrap_object_v1 WHERE workspace=?1",
            params![workspace_key(workspace())?],
            |row| row.get(0),
        )?;
        check_eq!(confirmed, 0);
        drop(connection);
        catalog.confirm_bootstrap(stage, &profile)?;
        let wrong = view(0, object(235, vec![]).reference, None);
        check!(matches!(
            catalog.publish_bootstrap(stage, wrong, &mut admission_budget()),
            Err(IndexError::BackendViolation(_))
        ));
        check_eq!(
            catalog.publish_bootstrap(
                forged,
                view(0, profile.reference, None),
                &mut admission_budget()
            ),
            Err(IndexError::FenceLost)
        );
        let connection = Connection::open(catalog.path())?;
        let heads: i64 = connection.query_row(
            "SELECT COUNT(*) FROM native_workspace_v1 WHERE workspace=?1",
            params![workspace_key(workspace())?],
            |row| row.get(0),
        )?;
        check_eq!(heads, 0);
        Ok(())
    }

    #[test]
    fn same_profile_digest_in_two_workspaces_retains_each_bootstrap_root() -> TestResult {
        let root = tempfile::tempdir()?;
        let catalog = open(root.path())?;
        let profile = object(236, vec![]);
        let first = catalog.begin_bootstrap(
            workspace(),
            7,
            &GenesisIdentity(profile.clone()),
            &mut admission_budget(),
        )?;
        catalog.reserve_bootstrap(first, &profile)?;
        catalog.confirm_bootstrap(first, &profile)?;

        let second_workspace = WorkspaceKey {
            slot: SlotId(id(237)),
            ..workspace()
        };
        let second = catalog.begin_bootstrap(
            second_workspace,
            8,
            &GenesisIdentity(profile.clone()),
            &mut admission_budget(),
        )?;
        let connection = Connection::open(catalog.path())?;
        let staged_profile: Vec<u8> = connection.query_row(
            "SELECT profile FROM native_bootstrap_v1 WHERE workspace=?1",
            params![workspace_key(second_workspace)?],
            |row| row.get(0),
        )?;
        check_eq!(staged_profile, object_key(profile.reference));
        drop(connection);

        let _published = catalog.publish_bootstrap(
            first,
            view(0, profile.reference, None),
            &mut admission_budget(),
        )?;
        drop(catalog);
        let catalog = open(root.path())?;
        check_eq!(
            catalog.begin_bootstrap(
                second_workspace,
                8,
                &GenesisIdentity(profile.clone()),
                &mut admission_budget(),
            )?,
            second
        );
        catalog.reserve_bootstrap(second, &profile)?;
        catalog.confirm_bootstrap(second, &profile)?;
        let mut genesis = view(0, profile.reference, None);
        genesis.identity.workspace = second_workspace;
        genesis.identity.epoch = 8;
        genesis.identity.digest = genesis.identity_digest()?;
        check_eq!(
            catalog.publish_bootstrap(second, genesis.clone(), &mut admission_budget())?,
            genesis.identity
        );
        let connection = Connection::open(catalog.path())?;
        let roots: i64 = connection.query_row(
            "SELECT COUNT(*) FROM native_view_root_v1 WHERE object=?1",
            params![object_key(profile.reference)],
            |row| row.get(0),
        )?;
        check_eq!(roots, 2);
        Ok(())
    }

    #[test]
    fn corrupt_bootstrap_stage_digest_or_profile_refuses_before_object_mutation() -> TestResult {
        for corrupt_profile in [false, true] {
            let root = tempfile::tempdir()?;
            let catalog = open(root.path())?;
            let profile = object(240, vec![]);
            let stage = catalog.begin_bootstrap(
                workspace(),
                7,
                &GenesisIdentity(profile.clone()),
                &mut admission_budget(),
            )?;
            let connection = Connection::open(catalog.path())?;
            let changed = if corrupt_profile {
                connection.execute(
                    "UPDATE native_bootstrap_v1 SET profile=zeroblob(32) WHERE workspace=?1",
                    params![workspace_key(workspace())?],
                )?
            } else {
                connection.execute(
                    "UPDATE native_bootstrap_v1 SET row_digest=zeroblob(32) WHERE workspace=?1",
                    params![workspace_key(workspace())?],
                )?
            };
            check_eq!(changed, 1);
            drop(connection);
            check!(matches!(
                catalog.reserve_bootstrap(stage, &profile),
                Err(IndexError::Storage(_))
            ));
            let connection = Connection::open(catalog.path())?;
            let intents: i64 = connection.query_row(
                "SELECT COUNT(*) FROM native_bootstrap_object_v1 WHERE workspace=?1",
                params![workspace_key(workspace())?],
                |row| row.get(0),
            )?;
            check_eq!(intents, 0);
        }
        Ok(())
    }

    static BOOTSTRAP_BUSY_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    static BOOTSTRAP_BUSY_SEEN: std::sync::atomic::AtomicBool =
        std::sync::atomic::AtomicBool::new(false);

    fn signal_bootstrap_busy(_prior_calls: i32) -> bool {
        BOOTSTRAP_BUSY_SEEN.store(true, Ordering::SeqCst);
        std::thread::yield_now();
        true
    }

    fn cancelled_original_budget_during_bootstrap_write_lock_wait(existing: bool) -> TestResult {
        let _serial = BOOTSTRAP_BUSY_TEST_LOCK
            .lock()
            .map_err(|poison| format!("bootstrap busy test lock poisoned: {poison}"))?;
        let root = tempfile::tempdir()?;
        let catalog = Arc::new(open(root.path())?);
        let profile = object(if existing { 247 } else { 248 }, vec![]);
        if existing {
            let _stage = catalog.begin_bootstrap(
                workspace(),
                7,
                &GenesisIdentity(profile.clone()),
                &mut admission_budget(),
            )?;
        }
        let state = |catalog: &SqliteCatalog| -> Result<(i64, i64), Box<dyn Error>> {
            let connection = Connection::open(catalog.path())?;
            let stages = connection.query_row(
                "SELECT COUNT(*) FROM native_bootstrap_v1 WHERE workspace=?1",
                params![workspace_key(workspace())?],
                |row| row.get(0),
            )?;
            let next = connection.query_row(
                "SELECT next FROM native_counter_v1 WHERE name='attempt_fence'",
                [],
                |row| row.get(0),
            )?;
            Ok((stages, next))
        };
        let before = state(&catalog)?;
        catalog.lock()?.busy_handler(Some(signal_bootstrap_busy))?;
        let blocker = Connection::open(catalog.path())?;
        blocker.execute_batch("BEGIN IMMEDIATE")?;
        BOOTSTRAP_BUSY_SEEN.store(false, Ordering::SeqCst);
        let budget = admission_budget();
        let cancel = budget.request().cancel_handle();
        let worker_catalog = Arc::clone(&catalog);
        let worker_profile = profile;
        let worker = std::thread::spawn(move || {
            let mut budget = budget;
            worker_catalog.begin_bootstrap(
                workspace(),
                7,
                &GenesisIdentity(worker_profile),
                &mut budget,
            )
        });
        let wait_until = Instant::now()
            .checked_add(Duration::from_secs(5))
            .ok_or("bootstrap busy wait deadline overflow")?;
        while !BOOTSTRAP_BUSY_SEEN.load(Ordering::SeqCst) && Instant::now() < wait_until {
            std::thread::yield_now();
        }
        let entered_busy = BOOTSTRAP_BUSY_SEEN.load(Ordering::SeqCst);
        cancel.cancel();
        blocker.execute_batch("ROLLBACK")?;
        let outcome = worker
            .join()
            .map_err(|panic| format!("bootstrap worker panicked: {panic:?}"))?;
        check!(entered_busy);
        check_eq!(outcome, Err(IndexError::Cancelled));
        check_eq!(state(&catalog)?, before);
        Ok(())
    }

    #[test]
    fn cancelled_original_budget_during_new_bootstrap_write_lock_wait_never_returns_stage()
    -> TestResult {
        cancelled_original_budget_during_bootstrap_write_lock_wait(false)
    }

    #[test]
    fn cancelled_original_budget_during_existing_bootstrap_write_lock_wait_never_returns_stage()
    -> TestResult {
        cancelled_original_budget_during_bootstrap_write_lock_wait(true)
    }

    #[test]
    fn corrupt_bootstrap_object_digest_refuses_publication_without_losing_custody() -> TestResult {
        let root = tempfile::tempdir()?;
        let catalog = open(root.path())?;
        let profile = object(241, vec![]);
        let stage = catalog.begin_bootstrap(
            workspace(),
            7,
            &GenesisIdentity(profile.clone()),
            &mut admission_budget(),
        )?;
        catalog.reserve_bootstrap(stage, &profile)?;
        catalog.confirm_bootstrap(stage, &profile)?;
        let connection = Connection::open(catalog.path())?;
        check_eq!(
            connection.execute(
                "UPDATE native_bootstrap_object_v1 SET row_digest=zeroblob(32)
                 WHERE workspace=?1 AND object=?2",
                params![workspace_key(workspace())?, object_key(profile.reference)],
            )?,
            1
        );
        drop(connection);

        check!(matches!(
            catalog.publish_bootstrap(
                stage,
                view(0, profile.reference, None),
                &mut admission_budget(),
            ),
            Err(IndexError::Storage(_))
        ));
        let connection = Connection::open(catalog.path())?;
        let heads: i64 = connection.query_row(
            "SELECT COUNT(*) FROM native_workspace_v1 WHERE workspace=?1",
            params![workspace_key(workspace())?],
            |row| row.get(0),
        )?;
        let stages: i64 = connection.query_row(
            "SELECT COUNT(*) FROM native_bootstrap_v1 WHERE workspace=?1",
            params![workspace_key(workspace())?],
            |row| row.get(0),
        )?;
        let confirmed: i64 = connection.query_row(
            "SELECT confirmed FROM native_bootstrap_object_v1 WHERE workspace=?1",
            params![workspace_key(workspace())?],
            |row| row.get(0),
        )?;
        let roots: i64 = connection.query_row(
            "SELECT COUNT(*) FROM native_view_root_v1 WHERE workspace=?1",
            params![workspace_key(workspace())?],
            |row| row.get(0),
        )?;
        check_eq!((heads, stages, confirmed, roots), (0, 1, 1, 0));
        Ok(())
    }

    #[test]
    fn cancelled_original_bootstrap_budget_cannot_publish_and_retry_keeps_stage() -> TestResult {
        let root = tempfile::tempdir()?;
        let catalog = open(root.path())?;
        let profile = object(242, vec![]);
        let genesis = view(0, profile.reference, None);
        let mut budget = admission_budget();
        let stage = catalog.begin_bootstrap(
            workspace(),
            7,
            &GenesisIdentity(profile.clone()),
            &mut budget,
        )?;
        catalog.reserve_bootstrap(stage, &profile)?;
        catalog.confirm_bootstrap(stage, &profile)?;
        budget.request().cancel_handle().cancel();
        check_eq!(
            catalog.publish_bootstrap(stage, genesis.clone(), &mut budget),
            Err(IndexError::Cancelled)
        );
        drop(catalog);

        let catalog = open(root.path())?;
        let connection = Connection::open(catalog.path())?;
        let heads: i64 = connection.query_row(
            "SELECT COUNT(*) FROM native_workspace_v1 WHERE workspace=?1",
            params![workspace_key(workspace())?],
            |row| row.get(0),
        )?;
        let stages: i64 = connection.query_row(
            "SELECT COUNT(*) FROM native_bootstrap_v1 WHERE workspace=?1",
            params![workspace_key(workspace())?],
            |row| row.get(0),
        )?;
        let confirmed: i64 = connection.query_row(
            "SELECT confirmed FROM native_bootstrap_object_v1 WHERE workspace=?1",
            params![workspace_key(workspace())?],
            |row| row.get(0),
        )?;
        let roots: i64 = connection.query_row(
            "SELECT COUNT(*) FROM native_view_root_v1 WHERE workspace=?1",
            params![workspace_key(workspace())?],
            |row| row.get(0),
        )?;
        check_eq!((heads, stages, confirmed, roots), (0, 1, 1, 0));
        drop(connection);
        check_eq!(
            catalog.publish_bootstrap(stage, genesis.clone(), &mut admission_budget())?,
            genesis.identity
        );
        let connection = Connection::open(catalog.path())?;
        let heads: i64 = connection.query_row(
            "SELECT COUNT(*) FROM native_workspace_v1 WHERE workspace=?1",
            params![workspace_key(workspace())?],
            |row| row.get(0),
        )?;
        let stages: i64 = connection.query_row(
            "SELECT COUNT(*) FROM native_bootstrap_v1 WHERE workspace=?1",
            params![workspace_key(workspace())?],
            |row| row.get(0),
        )?;
        check_eq!((heads, stages), (1, 0));
        Ok(())
    }

    #[test]
    fn gc_referenced_live_candidate_respects_original_zero_write_budget() -> TestResult {
        use quanta_index_core::index::ports::{CatalogReclaimPort, ManagedObjectAuthority};
        use quanta_index_core::index::{ObjectNamespaceBinding, ReclaimCandidate, ReclaimKind};

        let root = tempfile::tempdir()?;
        let catalog = open(root.path())?;
        catalog.bind_namespace(ObjectNamespaceBinding {
            root_device: 1,
            root_inode: 2,
            objects_device: 1,
            objects_inode: 3,
            lease_device: 1,
            lease_inode: 4,
        })?;
        let child = object(248, vec![]);
        let parent = object(249, vec![child.reference]);
        let before = {
            let mut guard = catalog.lock().map_err(|error| error.to_string())?;
            let tx = begin(&mut guard)?;
            reclaim::reserve_object(&tx, child.reference)?;
            object_install::reserve(&tx, &child)?;
            register_confirmed(&tx, std::slice::from_ref(&child))?;
            reclaim::confirm_object(&tx, child.reference)?;
            reclaim::reserve_object(&tx, parent.reference)?;
            object_install::reserve(&tx, &parent)?;
            register_confirmed(&tx, std::slice::from_ref(&parent))?;
            reclaim::confirm_object(&tx, parent.reference)?;
            // Releasing one custody scope queues the child while the parent's
            // registered edge still protects it. No lifecycle row is forged.
            reclaim::release(&tx, child.reference)?;
            let before: (i64, i64) = tx.query_row(
                "SELECT generation,phase FROM native_gc_object_v1 WHERE object=?1",
                params![object_key(child.reference)],
                |record| Ok((record.get(0)?, record.get(1)?)),
            )?;
            let queued: i64 = tx.query_row(
                "SELECT COUNT(*) FROM native_gc_queue_v1 WHERE object=?1",
                params![object_key(child.reference)],
                |record| record.get(0),
            )?;
            check_eq!(before.1, 0);
            check_eq!(queued, 1);
            tx.commit()?;
            drop(guard);
            before
        };
        let candidate = ReclaimCandidate {
            object: child.reference,
            generation: u64::try_from(before.0)?,
            kind: ReclaimKind::Registered,
        };
        let deadline = Instant::now()
            .checked_add(Duration::from_secs(30))
            .ok_or("test deadline overflow")?;
        let mut budget = WorkBudget::new(
            WorkLimits {
                reads: 100,
                writes: 0,
                bytes: 4096,
                mutations: 0,
            },
            deadline,
        );
        check_eq!(
            catalog.retire_candidate(candidate, &mut budget),
            Err(IndexError::ResourceLimited)
        );
        let guard = catalog.lock().map_err(|error| error.to_string())?;
        let after: (i64, i64) = guard.query_row(
            "SELECT generation,phase FROM native_gc_object_v1 WHERE object=?1",
            params![object_key(child.reference)],
            |record| Ok((record.get(0)?, record.get(1)?)),
        )?;
        let queued: i64 = guard.query_row(
            "SELECT COUNT(*) FROM native_gc_queue_v1 WHERE object=?1",
            params![object_key(child.reference)],
            |record| record.get(0),
        )?;
        check_eq!(after, before);
        check_eq!(queued, 1);
        drop(guard);
        Ok(())
    }

    #[test]
    fn gc_duplicate_child_registration_drains_one_physical_edge_after_reopen() -> TestResult {
        use quanta_index_core::index::ports::{CatalogReclaimPort, ManagedObjectAuthority};
        use quanta_index_core::index::{
            ObjectNamespaceBinding, PhysicalIncarnation, ReclaimTarget, ReclaimWork,
        };

        let root = tempfile::tempdir()?;
        let binding = ObjectNamespaceBinding {
            root_device: 1,
            root_inode: 2,
            objects_device: 1,
            objects_inode: 3,
            lease_device: 1,
            lease_inode: 4,
        };
        let catalog = open(root.path())?;
        catalog.bind_namespace(binding)?;
        let child = object(252, vec![]);
        let parent = object(253, vec![child.reference, child.reference]);
        {
            let mut guard = catalog.lock().map_err(|error| error.to_string())?;
            let tx = begin(&mut guard)?;
            reclaim::reserve_object(&tx, child.reference)?;
            object_install::reserve(&tx, &child)?;
            register_confirmed(&tx, std::slice::from_ref(&child))?;
            reclaim::confirm_object(&tx, child.reference)?;
            reclaim::reserve_object(&tx, parent.reference)?;
            object_install::reserve(&tx, &parent)?;
            register_confirmed(&tx, std::slice::from_ref(&parent))?;
            reclaim::confirm_object(&tx, parent.reference)?;
            let queued = reclaim::enqueue(&tx, parent.reference)?;
            check!(queued);
            tx.commit()?;
            drop(guard);
        }
        let distinct_edges: i64 = catalog.lock()?.query_row(
            "SELECT COUNT(*) FROM native_object_edge_v1 WHERE parent=?1",
            params![object_key(parent.reference)],
            |row| row.get(0),
        )?;
        check_eq!(distinct_edges, 1);
        let page = catalog.reclaim_work_page(None, 1, &mut admission_budget())?;
        let Some(ReclaimWork::Candidate(candidate)) = page.items.first().cloned() else {
            return Err("duplicate-child parent is not reclaim candidate".into());
        };
        check_eq!(candidate.object, parent.reference);
        let retiring = catalog
            .retire_candidate(candidate, &mut admission_budget())?
            .ok_or("duplicate-child parent unexpectedly retained")?;
        check_eq!(retiring.direct_children, 1);
        let permit = catalog.enter_reclaiming(
            retiring,
            ReclaimTarget::Present {
                incarnation: PhysicalIncarnation {
                    namespace: binding,
                    shard_device: 1,
                    shard_inode: 5,
                    device: 1,
                    inode: 6,
                    changed_nanos: 1,
                    bytes: parent.bytes,
                },
                registration: parent,
            },
            &mut admission_budget(),
        )?;
        drop(catalog);
        let catalog = open(root.path())?;
        catalog.bind_namespace(binding)?;
        catalog.validate_reclaiming(permit.clone(), &mut admission_budget())?;
        catalog.mark_unlinked(permit.clone(), &mut admission_budget())?;
        let drained = catalog.drain_unlinked(permit, 1, &mut admission_budget())?;
        check_eq!(drained.released_children, 1);
        check_eq!(drained.enqueued_children, 1);
        check!(!drained.has_more);
        let next = catalog.reclaim_work_page(None, 1, &mut admission_budget())?;
        check_eq!(next.items.len(), 1);
        let Some(ReclaimWork::Candidate(child_candidate)) = next.items.first() else {
            return Err("released duplicate child was not queued for reclaim".into());
        };
        check_eq!(child_candidate.object, child.reference);
        check!(!next.has_more);
        Ok(())
    }

    #[test]
    fn gc_4096_child_drain_advances_in_bounded_pages_across_restart() -> TestResult {
        use quanta_index_core::index::ports::{CatalogReclaimPort, ManagedObjectAuthority};
        use quanta_index_core::index::{
            ObjectNamespaceBinding, PhysicalIncarnation, ReclaimTarget, ReclaimWork,
        };

        let root = tempfile::tempdir()?;
        let binding = ObjectNamespaceBinding {
            root_device: 1,
            root_inode: 2,
            objects_device: 1,
            objects_inode: 3,
            lease_device: 1,
            lease_inode: 4,
        };
        let catalog = open(root.path())?;
        catalog.bind_namespace(binding)?;
        let mut children = Vec::with_capacity(4096);
        let mut leaves = Vec::with_capacity(4096);
        for number in 0_u32..4096 {
            let mut key = [0_u8; 32];
            let [high, mid_high, mid_low, low] = number.to_be_bytes();
            key.get_mut(..5)
                .ok_or("child object key prefix missing")?
                .copy_from_slice(&[1, high, mid_high, mid_low, low]);
            let reference = ObjectRef(Digest(key));
            children.push(reference);
            leaves.push(RegisteredObject::unscoped(reference, 1, Vec::new()));
        }
        let parent = RegisteredObject::unscoped(ObjectRef(Digest([0_u8; 32])), 1, children);
        {
            let mut guard = catalog.lock().map_err(|error| error.to_string())?;
            let tx = begin(&mut guard)?;
            for leaf in &leaves {
                reclaim::reserve_object(&tx, leaf.reference)?;
                object_install::reserve(&tx, leaf)?;
            }
            register_confirmed(&tx, &leaves)?;
            for leaf in &leaves {
                reclaim::confirm_object(&tx, leaf.reference)?;
                let _queued = reclaim::enqueue(&tx, leaf.reference)?;
            }
            reclaim::reserve_object(&tx, parent.reference)?;
            object_install::reserve(&tx, &parent)?;
            register_confirmed(&tx, std::slice::from_ref(&parent))?;
            reclaim::confirm_object(&tx, parent.reference)?;
            let _queued = reclaim::enqueue(&tx, parent.reference)?;
            tx.commit()?;
            drop(guard);
        }
        let work = catalog.reclaim_work_page(None, 1, &mut admission_budget())?;
        let Some(ReclaimWork::Candidate(candidate)) = work.items.first().cloned() else {
            return Err("4096-child parent was not first candidate".into());
        };
        check_eq!(candidate.object, parent.reference);
        let retiring = catalog
            .retire_candidate(candidate, &mut admission_budget())?
            .ok_or("4096-child parent unexpectedly retained")?;
        check_eq!(retiring.direct_children, 4096);
        let permit = catalog.enter_reclaiming(
            retiring,
            ReclaimTarget::Present {
                incarnation: PhysicalIncarnation {
                    namespace: binding,
                    shard_device: 1,
                    shard_inode: 6,
                    device: 1,
                    inode: 7,
                    changed_nanos: 123,
                    bytes: 1,
                },
                registration: parent.clone(),
            },
            &mut admission_budget(),
        )?;
        catalog.mark_unlinked(permit.clone(), &mut admission_budget())?;
        for page_number in 0..128 {
            let deadline = Instant::now()
                .checked_add(Duration::from_secs(30))
                .ok_or("test deadline overflow")?;
            let mut budget = WorkBudget::new(
                WorkLimits {
                    reads: 10_000,
                    writes: 256,
                    bytes: 4 * 1024 * 1024,
                    mutations: 256,
                },
                deadline,
            );
            let page = catalog.drain_unlinked(permit.clone(), 32, &mut budget)?;
            check_eq!(page.released_children, 32);
            check_eq!(page.has_more, page_number != 127);
            if page_number == 63 {
                drop(catalog);
                let reopened = open(root.path())?;
                reopened.bind_namespace(binding)?;
                // A durable Draining permit survives restart; this test's
                // connection handle is swapped below via shadowing.
                return finish_4096_drain_after_restart(&reopened, permit, &parent, root);
            }
        }
        Err("4096-child test did not exercise restart".into())
    }

    fn finish_4096_drain_after_restart(
        catalog: &SqliteCatalog,
        permit: quanta_index_core::index::ReclaimPermit,
        parent: &RegisteredObject,
        root: tempfile::TempDir,
    ) -> TestResult {
        use quanta_index_core::index::ports::CatalogReclaimPort;
        for page_number in 64..128 {
            let deadline = Instant::now()
                .checked_add(Duration::from_secs(30))
                .ok_or("test deadline overflow")?;
            let mut budget = WorkBudget::new(
                WorkLimits {
                    reads: 10_000,
                    writes: 256,
                    bytes: 4 * 1024 * 1024,
                    mutations: 256,
                },
                deadline,
            );
            let page = catalog.drain_unlinked(permit.clone(), 32, &mut budget)?;
            check_eq!(page.released_children, 32);
            check_eq!(page.has_more, page_number != 127);
        }
        check!(
            catalog
                .validate_reclaiming(permit.clone(), &mut admission_budget())
                .is_err()
        );
        check!(
            catalog
                .retire_candidate(permit.retiring.candidate, &mut admission_budget())?
                .is_none()
        );
        {
            let mut guard = catalog.lock().map_err(|error| error.to_string())?;
            let tx = begin(&mut guard)?;
            reclaim::reserve_object(&tx, parent.reference)?;
            object_install::reserve(&tx, parent)?;
            register_confirmed(&tx, std::slice::from_ref(parent))?;
            reclaim::confirm_object(&tx, parent.reference)?;
            let _queued = reclaim::enqueue(&tx, parent.reference)?;
            tx.commit()?;
            drop(guard);
        }
        let work = catalog.reclaim_work_page(None, 1, &mut admission_budget())?;
        let Some(quanta_index_core::index::ReclaimWork::Candidate(fresh)) =
            work.items.first().cloned()
        else {
            return Err("reinstalled parent is not candidate".into());
        };
        check_eq!(fresh.object, parent.reference);
        check_ne!(fresh.generation, permit.retiring.candidate.generation);
        check!(
            catalog
                .validate_reclaiming(permit, &mut admission_budget())
                .is_err()
        );
        drop(root);
        Ok(())
    }

    fn managed_adoption_fixture()
    -> Result<(tempfile::TempDir, SqliteCatalog, IngressRef), Box<dyn Error>> {
        use quanta_index_core::index::ports::{ManagedObjectAuthority, ManagedObjectInstallPort};
        use quanta_index_core::index::{ObjectInstallScope, ObjectNamespaceBinding};

        let root = tempfile::tempdir()?;
        let catalog = open(root.path())?;
        catalog.bind_namespace(ObjectNamespaceBinding {
            root_device: 1,
            root_inode: 2,
            objects_device: 1,
            objects_inode: 3,
            lease_device: 1,
            lease_inode: 4,
        })?;
        let profile = object(30, vec![]);
        let genesis = view(0, profile.reference, None);
        let stage = catalog.begin_bootstrap(
            genesis.identity.workspace,
            genesis.identity.epoch,
            &GenesisIdentity(profile.clone()),
            &mut admission_budget(),
        )?;
        catalog.reserve_managed(
            ObjectInstallScope::Bootstrap(stage),
            std::slice::from_ref(&profile),
            &mut admission_budget(),
        )?;
        catalog.confirm_managed(
            ObjectInstallScope::Bootstrap(stage),
            std::slice::from_ref(&profile),
            &mut admission_budget(),
        )?;
        let _changed = catalog.publish_bootstrap(stage, genesis, &mut admission_budget())?;
        let ingress = catalog.open_ingress(operation(12001, 151).key)?;
        Ok((root, catalog, ingress))
    }

    #[test]
    fn managed_namespace_binding_fused_read_keeps_shape_and_mutation_checks() -> TestResult {
        static NAMESPACE_READS: AtomicU64 = AtomicU64::new(0);

        fn count_namespace_reads(event: rusqlite::trace::TraceEvent<'_>) {
            if let rusqlite::trace::TraceEvent::Profile(statement, _duration) = event
                && (statement
                    .sql()
                    .starts_with("SELECT COUNT(*) FROM native_gc_namespace_v1")
                    || statement
                        .sql()
                        .starts_with("SELECT length(payload),length(row_digest),"))
            {
                let _prior = NAMESPACE_READS.fetch_add(1, Ordering::SeqCst);
            }
        }

        let (_root, catalog, _ingress) = managed_adoption_fixture()?;
        let guard = catalog.lock().map_err(|error| error.to_string())?;
        let original: (Vec<u8>, Vec<u8>) = guard.query_row(
            "SELECT payload,row_digest FROM native_gc_namespace_v1 WHERE singleton=1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        let binding = reclaim::bound_namespace(&guard)?;
        NAMESPACE_READS.store(0, Ordering::SeqCst);
        guard.trace_v2(
            rusqlite::trace::TraceEventCodes::SQLITE_TRACE_PROFILE,
            Some(count_namespace_reads),
        );
        check!(reclaim::is_bound(&guard)?);
        check_eq!(NAMESPACE_READS.load(Ordering::SeqCst), 2);
        reclaim::require_bound(&guard)?;
        guard.trace_v2(rusqlite::trace::TraceEventCodes::empty(), None);
        check_eq!(NAMESPACE_READS.load(Ordering::SeqCst), 3);
        check_eq!(reclaim::bound_namespace(&guard)?, binding);

        let _changed = guard.execute(
            "UPDATE native_gc_namespace_v1 SET payload=zeroblob(1000000) WHERE singleton=1",
            [],
        )?;
        check!(matches!(
            reclaim::require_bound(&guard),
            Err(IndexError::Storage(message)) if message == "managed namespace binding shape mismatch"
        ));
        let _changed = guard.execute(
            "UPDATE native_gc_namespace_v1 SET payload=?1,row_digest=zeroblob(31) WHERE singleton=1",
            params![&original.0],
        )?;
        check!(matches!(
            reclaim::require_bound(&guard),
            Err(IndexError::Storage(message)) if message == "managed namespace binding shape mismatch"
        ));
        let _changed = guard.execute(
            "UPDATE native_gc_namespace_v1 SET payload='not json',row_digest=zeroblob(32) WHERE singleton=1",
            [],
        )?;
        check!(matches!(
            reclaim::require_bound(&guard),
            Err(IndexError::Storage(_))
        ));
        let mut truncated = original.0.clone();
        let _removed = truncated.pop().ok_or("empty namespace payload")?;
        let _changed = guard.execute(
            "UPDATE native_gc_namespace_v1 SET payload=?1,row_digest=?2 WHERE singleton=1",
            params![
                &truncated,
                digest(b"qidx.native.gc.namespace.v1", &truncated).to_vec()
            ],
        )?;
        check!(matches!(
            reclaim::require_bound(&guard),
            Err(IndexError::Storage(message)) if message.contains("decode native catalog row")
        ));
        let malformed = [0_u8];
        let _changed = guard.execute(
            "UPDATE native_gc_namespace_v1 SET payload=?1,row_digest=?2 WHERE singleton=1",
            params![
                malformed.as_slice(),
                digest(b"qidx.native.gc.namespace.v1", &malformed).to_vec()
            ],
        )?;
        check!(matches!(
            reclaim::require_bound(&guard),
            Err(IndexError::Storage(message)) if message.contains("decode native catalog row")
        ));
        let _changed = guard.execute(
            "UPDATE native_gc_namespace_v1 SET payload=?1,row_digest=zeroblob(32) WHERE singleton=1",
            params![&original.0],
        )?;
        check!(matches!(
            reclaim::require_bound(&guard),
            Err(IndexError::Storage(message)) if message == "native catalog row digest mismatch"
        ));
        let _changed = guard.execute(
            "UPDATE native_gc_namespace_v1 SET payload=?1,row_digest=?2 WHERE singleton=1",
            params![&original.0, &original.1],
        )?;
        check_eq!(reclaim::bound_namespace(&guard)?, binding);

        guard.execute_batch(
            "CREATE TEMP TABLE poison_namespace_once(marker INTEGER);
             CREATE TEMP TRIGGER poison_namespace_after_insert
             AFTER INSERT ON poison_namespace_once
             BEGIN UPDATE native_gc_namespace_v1 SET row_digest=zeroblob(32) WHERE singleton=1; END",
        )?;
        let _changed = guard.execute("INSERT INTO poison_namespace_once(marker) VALUES(1)", [])?;
        check!(matches!(
            reclaim::require_bound(&guard),
            Err(IndexError::Storage(message)) if message == "native catalog row digest mismatch"
        ));
        let _changed = guard.execute("DELETE FROM native_gc_namespace_v1 WHERE singleton=1", [])?;
        check!(matches!(
            reclaim::require_bound(&guard),
            Err(IndexError::BackendViolation(message)) if message == "managed namespace is unbound"
        ));
        check!(!reclaim::is_bound(&guard)?);
        drop(guard);
        Ok(())
    }

    #[test]
    fn managed_adopt_registered_inputs_requires_exact_confirmation_fence_and_budget() -> TestResult
    {
        use quanta_index_core::index::ObjectInstallScope;
        use quanta_index_core::index::ports::{InputStagingPort, ManagedObjectInstallPort};

        let (_root, catalog, ingress) = managed_adoption_fixture()?;
        let confirmed = object(152, vec![]);
        let provisional = object(153, vec![]);
        let donor = catalog.open_ingress(operation(12002, 152).key)?;
        catalog.reserve_managed(
            ObjectInstallScope::Ingress(donor),
            std::slice::from_ref(&confirmed),
            &mut admission_budget(),
        )?;
        catalog.confirm_managed(
            ObjectInstallScope::Ingress(donor),
            std::slice::from_ref(&confirmed),
            &mut admission_budget(),
        )?;
        let forged = IngressRef {
            fence: ingress.fence.checked_add(1).ok_or("test fence overflow")?,
            ..ingress
        };
        check_eq!(
            catalog.adopt_registered_inputs(forged, &[], &mut admission_budget()),
            Err(IndexError::FenceLost)
        );
        catalog.adopt_registered_inputs(ingress, &[], &mut admission_budget())?;
        let deadline = Instant::now()
            .checked_add(Duration::from_secs(30))
            .ok_or("test deadline overflow")?;
        let mut insufficient_writes = WorkBudget::new(
            WorkLimits {
                reads: 100_000,
                writes: 3,
                bytes: 32 * 1024 * 1024,
                mutations: 3,
            },
            deadline,
        );
        check_eq!(
            catalog.adopt_registered_inputs(
                ingress,
                std::slice::from_ref(&confirmed),
                &mut insufficient_writes,
            ),
            Err(IndexError::ResourceLimited)
        );
        {
            let guard = catalog.lock().map_err(|error| error.to_string())?;
            check!(!confirmed_ingress_object(
                &guard,
                ingress.operation,
                confirmed.reference
            )?);
        }
        let mut cancelled = admission_budget();
        cancelled.request().cancel_handle().cancel();
        check_eq!(
            catalog.adopt_registered_inputs(
                ingress,
                std::slice::from_ref(&confirmed),
                &mut cancelled,
            ),
            Err(IndexError::Cancelled)
        );
        let unknown = object(154, vec![]);
        check!(
            catalog
                .adopt_registered_inputs(
                    ingress,
                    std::slice::from_ref(&unknown),
                    &mut admission_budget()
                )
                .is_err()
        );
        catalog.reserve_managed(
            ObjectInstallScope::Ingress(ingress),
            std::slice::from_ref(&provisional),
            &mut admission_budget(),
        )?;
        check!(
            catalog
                .adopt_registered_inputs(
                    ingress,
                    std::slice::from_ref(&provisional),
                    &mut admission_budget(),
                )
                .is_err()
        );
        let changed = RegisteredObject::unscoped(
            (confirmed.clone()).reference,
            2,
            (confirmed.clone()).children.clone(),
        );
        check!(
            catalog
                .adopt_registered_inputs(
                    ingress,
                    std::slice::from_ref(&changed),
                    &mut admission_budget()
                )
                .is_err()
        );
        {
            let guard = catalog.lock().map_err(|error| error.to_string())?;
            check!(!confirmed_ingress_object(
                &guard,
                ingress.operation,
                unknown.reference
            )?);
            check!(!confirmed_ingress_object(
                &guard,
                ingress.operation,
                provisional.reference
            )?);
        }
        catalog.adopt_registered_inputs(
            ingress,
            std::slice::from_ref(&confirmed),
            &mut admission_budget(),
        )?;
        let guard = catalog.lock().map_err(|error| error.to_string())?;
        check!(confirmed_ingress_object(
            &guard,
            ingress.operation,
            confirmed.reference
        )?);
        Ok(())
    }

    #[test]
    fn managed_adopt_registered_inputs_cancels_retiring_and_refuses_reclaiming() -> TestResult {
        use quanta_index_core::index::ports::{CatalogReclaimPort, InputStagingPort};
        use quanta_index_core::index::{PhysicalIncarnation, ReclaimTarget, ReclaimWork};

        let (_root, catalog, ingress) = managed_adoption_fixture()?;
        let rescued = object(155, vec![]);
        let blocked = object(156, vec![]);
        {
            let mut guard = catalog.lock().map_err(|error| error.to_string())?;
            let tx = begin(&mut guard)?;
            for object in [&rescued, &blocked] {
                reclaim::reserve_object(&tx, object.reference)?;
                object_install::reserve(&tx, object)?;
                register_confirmed(&tx, std::slice::from_ref(object))?;
                reclaim::confirm_object(&tx, object.reference)?;
                let _changed = reclaim::enqueue(&tx, object.reference)?;
            }
            tx.commit()?;
            drop(guard);
        }
        let page = catalog.reclaim_work_page(None, 8, &mut admission_budget())?;
        let candidates = page
            .items
            .iter()
            .filter_map(|work| match work {
                ReclaimWork::Candidate(candidate) => Some(*candidate),
                ReclaimWork::Reclaiming(_) | ReclaimWork::Draining(_) => None,
            })
            .collect::<Vec<_>>();
        let rescue_candidate = candidates
            .iter()
            .find(|candidate| candidate.object == rescued.reference)
            .copied()
            .ok_or("rescued candidate missing")?;
        let blocked_candidate = candidates
            .iter()
            .find(|candidate| candidate.object == blocked.reference)
            .copied()
            .ok_or("blocked candidate missing")?;
        let rescued_retiring = catalog
            .retire_candidate(rescue_candidate, &mut admission_budget())?
            .ok_or("rescued object was not retired")?;
        let blocked_retiring = catalog
            .retire_candidate(blocked_candidate, &mut admission_budget())?
            .ok_or("blocked object was not retired")?;
        let binding = quanta_index_core::index::ObjectNamespaceBinding {
            root_device: 1,
            root_inode: 2,
            objects_device: 1,
            objects_inode: 3,
            lease_device: 1,
            lease_inode: 4,
        };
        let _permit = catalog.enter_reclaiming(
            blocked_retiring,
            ReclaimTarget::Present {
                incarnation: PhysicalIncarnation {
                    namespace: binding,
                    shard_device: 1,
                    shard_inode: 6,
                    device: 1,
                    inode: 7,
                    changed_nanos: 123,
                    bytes: blocked.bytes,
                },
                registration: blocked.clone(),
            },
            &mut admission_budget(),
        )?;
        catalog.adopt_registered_inputs(
            ingress,
            std::slice::from_ref(&rescued),
            &mut admission_budget(),
        )?;
        check_eq!(
            catalog.retire_candidate(rescue_candidate, &mut admission_budget())?,
            None
        );
        check!(
            catalog
                .adopt_registered_inputs(
                    ingress,
                    std::slice::from_ref(&blocked),
                    &mut admission_budget(),
                )
                .is_err()
        );
        let guard = catalog.lock().map_err(|error| error.to_string())?;
        check!(confirmed_ingress_object(
            &guard,
            ingress.operation,
            rescued.reference
        )?);
        check!(!confirmed_ingress_object(
            &guard,
            ingress.operation,
            blocked.reference
        )?);
        let rescued_generation: i64 = guard.query_row(
            "SELECT generation FROM native_gc_object_v1 WHERE object=?1",
            params![object_key(rescued.reference)],
            |row| row.get(0),
        )?;
        check_ne!(
            u64::try_from(rescued_generation)?,
            rescued_retiring.candidate.generation
        );
        drop(guard);
        // Frozen lifecycle state is a refusal before target/digest materialization,
        // even if its untrusted payload exceeds the connection's reader limit.
        let connection = Connection::open(catalog.path())?;
        check_eq!(
            connection.execute(
                "UPDATE native_gc_object_v1 SET target=zeroblob(65536),row_digest=zeroblob(65536) WHERE object=?1",
                params![object_key(blocked.reference)],
            )?,
            1
        );
        let prior_limit = prechecked_children::reader_length_limit(&catalog, 2048)?;
        let blocked_result = catalog.adopt_registered_inputs(
            ingress,
            std::slice::from_ref(&blocked),
            &mut admission_budget(),
        );
        check_eq!(
            prechecked_children::reader_length_limit(&catalog, prior_limit)?,
            2048
        );
        check_eq!(blocked_result, Err(IndexError::Conflict));
        let lengths: (i64, i64) = connection.query_row(
            "SELECT length(target),length(row_digest) FROM native_gc_object_v1 WHERE object=?1",
            params![object_key(blocked.reference)],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        check_eq!(lengths, (65536, 65536));
        Ok(())
    }

    #[test]
    fn confirmed_batch_preflight_rejects_later_provisional_corruption_atomically() -> TestResult {
        for corrupt in [
            "UPDATE native_object_install_v1 SET row_digest=zeroblob(32) WHERE object=?1",
            "UPDATE native_object_install_edge_v1 SET row_digest=zeroblob(32) WHERE parent=?1",
        ] {
            let root = tempfile::tempdir()?;
            let catalog = open(root.path())?;
            let leaf = object(245, vec![]);
            let parent = object(246, vec![leaf.reference]);
            let batch = [leaf.clone(), parent.clone()];
            let mut guard = catalog.lock().map_err(|error| error.to_string())?;
            let tx = begin(&mut guard)?;
            for object in &batch {
                object_install::reserve(&tx, object)?;
            }
            tx.commit()?;
            check_eq!(
                guard.execute(corrupt, params![object_key(parent.reference)])?,
                1
            );
            let tx = begin(&mut guard)?;
            check!(matches!(
                register_confirmed(&tx, &batch),
                Err(IndexError::Storage(_))
            ));
            drop(tx);
            drop(guard);
            // A bad later row must leave both provisional registrations intact
            // and must not publish the earlier, otherwise valid leaf.
            check_eq!(
                provisional_child_rows(&catalog, leaf.reference, parent.reference)?,
                (1, 0, 0)
            );
            check_eq!(
                provisional_child_rows(&catalog, parent.reference, leaf.reference)?,
                (1, 1, 0)
            );
            let permanent: i64 =
                catalog
                    .lock()?
                    .query_row("SELECT COUNT(*) FROM native_object_v1", [], |row| {
                        row.get(0)
                    })?;
            check_eq!(permanent, 0);
        }
        Ok(())
    }

    #[test]
    fn confirmed_batch_transfer_preserves_duplicate_replay_and_durable_edges() -> TestResult {
        let root = tempfile::tempdir()?;
        let catalog = open(root.path())?;
        let leaf = object(245, vec![]);
        let parent = object(246, vec![leaf.reference]);
        let batch = [leaf.clone(), leaf.clone(), parent.clone(), parent.clone()];
        {
            let mut guard = catalog.lock().map_err(|error| error.to_string())?;
            let tx = begin(&mut guard)?;
            object_install::reserve(&tx, &leaf)?;
            object_install::reserve(&tx, &parent)?;
            tx.commit()?;
            // Valid pending objects would otherwise be confirmable here.
            check!(matches!(
                register_confirmed(&guard, &batch),
                Err(IndexError::BackendViolation(_))
            ));
            let tx = begin(&mut guard)?;
            register_confirmed(&tx, &batch)?;
            tx.commit()?;
            drop(guard);
        }
        drop(catalog);
        let catalog = open(root.path())?;
        {
            let mut guard = catalog.lock().map_err(|error| error.to_string())?;
            let tx = begin(&mut guard)?;
            register_confirmed(&tx, &batch)?;
            tx.commit()?;
            drop(guard);
        }
        check_eq!(
            provisional_child_rows(&catalog, parent.reference, leaf.reference)?,
            (0, 0, 1)
        );
        let guard = catalog.lock().map_err(|error| error.to_string())?;
        let permanent: i64 =
            guard.query_row("SELECT COUNT(*) FROM native_object_v1", [], |row| {
                row.get(0)
            })?;
        check_eq!(permanent, 2);
        let child: Vec<u8> = guard.query_row(
            "SELECT child FROM native_object_edge_v1 WHERE parent=?1",
            params![object_key(parent.reference)],
            |row| row.get(0),
        )?;
        check_eq!(child, object_key(leaf.reference));
        drop(guard);
        Ok(())
    }

    #[test]
    fn managed_attempt_child_preflight_preserves_integrity_atomicity_and_reopen() -> TestResult {
        use quanta_index_core::index::ObjectInstallScope;
        use quanta_index_core::index::ports::ManagedObjectInstallPort;

        let (root, catalog, ingress) = managed_adoption_fixture()?;
        let external = object(241, vec![object(30, vec![]).reference]);
        catalog.reserve_managed(
            ObjectInstallScope::Ingress(ingress),
            std::slice::from_ref(&external),
            &mut admission_budget(),
        )?;
        catalog.confirm_managed(
            ObjectInstallScope::Ingress(ingress),
            std::slice::from_ref(&external),
            &mut admission_budget(),
        )?;
        let op = operation(12002, 153);
        let request = RegisteredObject::unscoped(op.request, 1, vec![]);
        let request_ingress = catalog.open_ingress(op.key)?;
        catalog.reserve_managed(
            ObjectInstallScope::Ingress(request_ingress),
            std::slice::from_ref(&request),
            &mut admission_budget(),
        )?;
        catalog.confirm_managed(
            ObjectInstallScope::Ingress(request_ingress),
            std::slice::from_ref(&request),
            &mut admission_budget(),
        )?;
        catalog.seal_inputs(request_ingress, op.request)?;
        let attempt = match catalog.admit(op, request_ingress, &[], &mut admission_budget())? {
            AdmissionOutcome::New(attempt) => attempt,
            AdmissionOutcome::Existing(status) => {
                return Err(format!("expected fresh managed attempt, got {status:?}").into());
            }
        };
        let leaf = object(242, vec![]);
        let parent = object(243, vec![external.reference, leaf.reference]);
        let batch = [leaf.clone(), parent.clone()];
        let connection = Connection::open(catalog.path())?;
        let edge_digest: Vec<u8> = connection.query_row(
            "SELECT row_digest FROM native_object_edge_v1 WHERE parent=?1",
            params![object_key(external.reference)],
            |row| row.get(0),
        )?;
        check_eq!(
            connection.execute(
                "UPDATE native_object_edge_v1 SET row_digest=zeroblob(32) WHERE parent=?1",
                params![object_key(external.reference)],
            )?,
            1
        );
        check!(matches!(
            catalog.reserve_managed(
                ObjectInstallScope::Attempt(attempt),
                &batch,
                &mut admission_budget(),
            ),
            Err(IndexError::CorruptObject(_))
        ));
        // The later corrupt child must roll back even earlier GC reservation.
        for table in [
            "native_object_install_v1",
            "native_object_intent_v1",
            "native_gc_object_v1",
        ] {
            let count: i64 = connection.query_row(
                &format!("SELECT COUNT(*) FROM {table} WHERE object=?1"),
                params![object_key(leaf.reference)],
                |row| row.get(0),
            )?;
            check_eq!(count, 0);
        }
        check_eq!(
            connection.execute(
                "UPDATE native_object_edge_v1 SET row_digest=?1 WHERE parent=?2",
                params![edge_digest, object_key(external.reference)],
            )?,
            1
        );
        drop(connection);
        // Both public scopes authenticate the same external child once. The
        // attempt path must not bill the removed second validation in intent
        // insertion. Keep the existing corruption, rollback and reopen gold.
        let mut ingress_work = admission_budget();
        catalog.reserve_managed(
            ObjectInstallScope::Ingress(ingress),
            &batch,
            &mut ingress_work,
        )?;
        let mut attempt_work = admission_budget();
        catalog.reserve_managed(
            ObjectInstallScope::Attempt(attempt),
            &batch,
            &mut attempt_work,
        )?;
        let mut expected_attempt = ingress_work.usage();
        // One earlier child saves four adoption units. Both provisional slots
        // already exist here, so each pays two fallback units; the Attempt
        // intents are new. Ingress above created both slots and intents fresh.
        let fallback_reads = u64::try_from(batch.len())?
            .checked_mul(2)
            .ok_or("fallback read overflow")?;
        expected_attempt.reads = expected_attempt
            .reads
            .checked_sub(4)
            .and_then(|reads| reads.checked_add(fallback_reads))
            .ok_or("read charge overflow")?;
        check_eq!(attempt_work.usage(), expected_attempt);
        let changed = RegisteredObject::unscoped(parent.reference, 2, parent.children.clone());
        let changed_batch = [leaf.clone(), changed];
        check!(matches!(
            catalog.reserve_managed(
                ObjectInstallScope::Attempt(attempt),
                &changed_batch,
                &mut admission_budget(),
            ),
            Err(IndexError::CorruptObject(_))
        ));
        catalog.confirm_managed(
            ObjectInstallScope::Attempt(attempt),
            &batch,
            &mut admission_budget(),
        )?;
        // Non-unique attempt replay retains the full confirmation path.
        catalog.confirm_managed(
            ObjectInstallScope::Attempt(attempt),
            &[leaf.clone(), leaf.clone(), parent.clone(), parent.clone()],
            &mut admission_budget(),
        )?;
        drop(catalog);
        let catalog = open(root.path())?;
        catalog.reserve_managed(
            ObjectInstallScope::Attempt(attempt),
            &batch,
            &mut admission_budget(),
        )?;
        let guard = catalog.lock().map_err(|error| error.to_string())?;
        check!(object_install::confirmed_matches(&guard, &leaf)?);
        check!(object_install::confirmed_matches(&guard, &parent)?);
        drop(guard);
        // A new transaction must revalidate the external child, not reuse a cache.
        let connection = Connection::open(catalog.path())?;
        check_eq!(
            connection.execute(
                "UPDATE native_object_edge_v1 SET row_digest=zeroblob(32) WHERE parent=?1",
                params![object_key(external.reference)],
            )?,
            1
        );
        let next = object(244, vec![external.reference]);
        check!(matches!(
            catalog.reserve_managed(
                ObjectInstallScope::Attempt(attempt),
                std::slice::from_ref(&next),
                &mut admission_budget(),
            ),
            Err(IndexError::CorruptObject(_))
        ));
        check_eq!(
            provisional_child_rows(&catalog, next.reference, external.reference)?,
            (0, 0, 0)
        );
        Ok(())
    }

    #[test]
    fn gc_managed_ingress_batch_confirms_leaf_before_parent_in_one_transaction() -> TestResult {
        use quanta_index_core::index::ports::{ManagedObjectAuthority, ManagedObjectInstallPort};
        use quanta_index_core::index::{ObjectInstallScope, ObjectNamespaceBinding};

        let root = tempfile::tempdir()?;
        let catalog = open(root.path())?;
        let binding = ObjectNamespaceBinding {
            root_device: 1,
            root_inode: 2,
            objects_device: 1,
            objects_inode: 3,
            lease_device: 1,
            lease_inode: 4,
        };
        catalog.bind_namespace(binding)?;
        let profile = object(30, vec![]);
        let genesis = view(0, profile.reference, None);
        let stage = catalog.begin_bootstrap(
            genesis.identity.workspace,
            genesis.identity.epoch,
            &GenesisIdentity(profile.clone()),
            &mut admission_budget(),
        )?;
        catalog.reserve_managed(
            ObjectInstallScope::Bootstrap(stage),
            std::slice::from_ref(&profile),
            &mut admission_budget(),
        )?;
        catalog.confirm_managed(
            ObjectInstallScope::Bootstrap(stage),
            std::slice::from_ref(&profile),
            &mut admission_budget(),
        )?;
        let _view = catalog.publish_bootstrap(stage, genesis, &mut admission_budget())?;

        let op = operation(100, 99);
        let ingress = catalog.open_ingress(op.key)?;
        let leaf = object(90, vec![]);
        let parent = object(91, vec![leaf.reference]);
        let batch = [leaf.clone(), parent.clone()];
        catalog.reserve_managed(
            ObjectInstallScope::Ingress(ingress),
            &batch,
            &mut admission_budget(),
        )?;
        catalog.confirm_managed(
            ObjectInstallScope::Ingress(ingress),
            &batch,
            &mut admission_budget(),
        )?;
        let guard = catalog.lock().map_err(|error| error.to_string())?;
        check!(object_install::confirmed_matches(&guard, &leaf)?);
        check!(object_install::confirmed_matches(&guard, &parent)?);
        check!(confirmed_ingress_object(&guard, op.key, leaf.reference)?);
        check!(confirmed_ingress_object(&guard, op.key, parent.reference)?);
        let phase: i64 = guard.query_row(
            "SELECT phase FROM native_gc_object_v1 WHERE object=?1",
            params![object_key(parent.reference)],
            |row| row.get(0),
        )?;
        check_eq!(phase, 0);
        drop(guard);
        Ok(())
    }
    #[test]
    fn gc_aborted_provisional_bootstrap_is_queued_and_retiring_restarts() -> TestResult {
        use quanta_index_core::index::ports::{
            CatalogReclaimPort, ManagedObjectAuthority, ManagedObjectInstallPort,
        };
        use quanta_index_core::index::{
            ObjectInstallScope, ObjectNamespaceBinding, ReclaimKind, ReclaimWork,
        };

        let root = tempfile::tempdir()?;
        let binding = ObjectNamespaceBinding {
            root_device: 1,
            root_inode: 2,
            objects_device: 1,
            objects_inode: 3,
            lease_device: 1,
            lease_inode: 4,
        };
        let catalog = open(root.path())?;
        catalog.bind_namespace(binding)?;
        let profile = object(245, vec![]);
        let stage = catalog.begin_bootstrap(
            workspace(),
            7,
            &GenesisIdentity(profile.clone()),
            &mut admission_budget(),
        )?;
        catalog.reserve_managed(
            ObjectInstallScope::Bootstrap(stage),
            std::slice::from_ref(&profile),
            &mut admission_budget(),
        )?;
        catalog.abort_bootstrap(stage, &mut admission_budget())?;
        check!(matches!(
            catalog.confirm_managed(
                ObjectInstallScope::Bootstrap(stage),
                std::slice::from_ref(&profile),
                &mut admission_budget(),
            ),
            Err(IndexError::FenceLost)
        ));
        let page = catalog.reclaim_work_page(None, 1, &mut admission_budget())?;
        check_eq!(page.items.len(), 1);
        let Some(ReclaimWork::Candidate(candidate)) = page.items.first().cloned() else {
            return Err("aborted provisional object is not a candidate".into());
        };
        check_eq!(candidate.object, profile.reference);
        check_eq!(candidate.kind, ReclaimKind::Provisional);
        let retiring = catalog
            .retire_candidate(candidate, &mut admission_budget())?
            .ok_or("provisional object unexpectedly remained live")?;
        drop(catalog);

        let catalog = open(root.path())?;
        catalog.bind_namespace(binding)?;
        let resumed = catalog.reclaim_work_page(None, 1, &mut admission_budget())?;
        check_eq!(resumed.items, vec![ReclaimWork::Candidate(candidate)]);
        check_eq!(
            catalog.retire_candidate(candidate, &mut admission_budget())?,
            Some(retiring)
        );
        let replacement = catalog.begin_bootstrap(
            workspace(),
            7,
            &GenesisIdentity(profile.clone()),
            &mut admission_budget(),
        )?;
        check_ne!(replacement.fence, stage.fence);
        catalog.reserve_managed(
            ObjectInstallScope::Bootstrap(replacement),
            std::slice::from_ref(&profile),
            &mut admission_budget(),
        )?;
        check!(
            catalog
                .retire_candidate(candidate, &mut admission_budget())?
                .is_none()
        );
        Ok(())
    }

    #[test]
    fn gc_managed_install_rejects_wrong_frozen_bootstrap_object_without_debt() -> TestResult {
        use quanta_index_core::index::ports::{
            CatalogReclaimPort, ManagedObjectAuthority, ManagedObjectInstallPort,
        };
        use quanta_index_core::index::{ObjectInstallScope, ObjectNamespaceBinding};

        let root = tempfile::tempdir()?;
        let catalog = open(root.path())?;
        catalog.bind_namespace(ObjectNamespaceBinding {
            root_device: 1,
            root_inode: 2,
            objects_device: 1,
            objects_inode: 3,
            lease_device: 1,
            lease_inode: 4,
        })?;
        let profile = object(246, vec![]);
        let stage = catalog.begin_bootstrap(
            workspace(),
            7,
            &GenesisIdentity(profile.clone()),
            &mut admission_budget(),
        )?;
        let mut wrong = profile.clone();
        wrong = RegisteredObject::unscoped(wrong.reference, 2, wrong.children.clone());
        check!(
            catalog
                .reserve_managed(
                    ObjectInstallScope::Bootstrap(stage),
                    std::slice::from_ref(&wrong),
                    &mut admission_budget(),
                )
                .is_err()
        );
        let connection = Connection::open(catalog.path())?;
        let provisional: i64 = connection.query_row(
            "SELECT COUNT(*) FROM native_object_install_v1 WHERE object=?1",
            params![object_key(profile.reference)],
            |row| row.get(0),
        )?;
        check_eq!(provisional, 0);
        drop(connection);
        catalog.abort_bootstrap(stage, &mut admission_budget())?;
        check!(
            catalog
                .reclaim_work_page(None, 1, &mut admission_budget())?
                .items
                .is_empty()
        );
        Ok(())
    }

    #[derive(Clone, Copy)]
    enum BudgetedMutexRoute {
        Reclaim,
        ManagedInstall,
        BootstrapAbort,
        AdoptIngress,
    }

    fn assert_budgeted_native_mutex_wait(route: BudgetedMutexRoute, cancel: bool) -> TestResult {
        use quanta_index_core::index::ports::InputStagingPort;
        use std::sync::mpsc;

        let root = tempfile::tempdir()?;
        let catalog = Arc::new(open(root.path())?);
        let held = catalog.lock()?;
        let deadline = Instant::now()
            .checked_add(if cancel {
                Duration::from_secs(5)
            } else {
                Duration::from_millis(120)
            })
            .ok_or("deadline overflow")?;
        let request = quanta_index_core::RequestBudgetV1::until(deadline);
        let mut budget = WorkBudget::with_request(
            WorkLimits {
                reads: 100,
                writes: 100,
                bytes: 4096,
                mutations: 100,
            },
            request.clone(),
        );
        let (started_tx, started_rx) = mpsc::channel();
        let (finished_tx, finished_rx) = mpsc::channel();
        let worker_catalog = Arc::clone(&catalog);
        let worker = std::thread::spawn(move || {
            let _started = started_tx.send(());
            let profile = object(249, vec![]);
            let stage = BootstrapRef {
                workspace: workspace(),
                epoch: 7,
                fence: 1,
                profile: profile.reference,
            };
            let result = match route {
                BudgetedMutexRoute::Reclaim => worker_catalog
                    .retire_candidate(
                        ReclaimCandidate {
                            object: profile.reference,
                            generation: 1,
                            kind: quanta_index_core::index::ReclaimKind::Provisional,
                        },
                        &mut budget,
                    )
                    .map(|_candidate| ()),
                BudgetedMutexRoute::ManagedInstall => worker_catalog.reserve_managed(
                    ObjectInstallScope::Bootstrap(stage),
                    std::slice::from_ref(&profile),
                    &mut budget,
                ),
                BudgetedMutexRoute::BootstrapAbort => {
                    worker_catalog.abort_bootstrap(stage, &mut budget)
                }
                BudgetedMutexRoute::AdoptIngress => worker_catalog.adopt_registered_inputs(
                    IngressRef {
                        operation: operation(12003, 157).key,
                        fence: 1,
                    },
                    &[],
                    &mut budget,
                ),
            };
            let _finished = finished_tx.send(result);
        });
        let started = started_rx.recv_timeout(Duration::from_secs(1));
        if started.is_err() {
            drop(held);
            worker
                .join()
                .map_err(|panic| format!("budget worker panicked: {panic:?}"))?;
            return Err("budget worker did not start".into());
        }
        // Give the worker time to enter the blocked catalog call. The guard
        // stays held until after the bounded response oracle is observed.
        std::thread::sleep(Duration::from_millis(40));
        if cancel {
            request.cancel_handle().cancel();
        }
        let finished = finished_rx.recv_timeout(Duration::from_secs(2));
        drop(held);
        worker
            .join()
            .map_err(|panic| format!("budget worker panicked: {panic:?}"))?;
        let result = finished.map_err(|timeout| {
            format!("native catalog operation waited beyond original budget: {timeout}")
        })?;
        let expected = if cancel {
            IndexError::Cancelled
        } else {
            IndexError::ResourceLimited
        };
        check_eq!(result, Err(expected));
        Ok(())
    }

    #[test]
    fn cancelled_native_gc_and_install_do_not_wait_for_held_catalog_mutex() -> TestResult {
        assert_budgeted_native_mutex_wait(BudgetedMutexRoute::Reclaim, true)?;
        assert_budgeted_native_mutex_wait(BudgetedMutexRoute::ManagedInstall, true)?;
        assert_budgeted_native_mutex_wait(BudgetedMutexRoute::BootstrapAbort, true)?;
        assert_budgeted_native_mutex_wait(BudgetedMutexRoute::AdoptIngress, true)
    }

    #[test]
    fn expired_native_gc_and_bootstrap_do_not_wait_for_held_catalog_mutex() -> TestResult {
        assert_budgeted_native_mutex_wait(BudgetedMutexRoute::Reclaim, false)?;
        assert_budgeted_native_mutex_wait(BudgetedMutexRoute::BootstrapAbort, false)?;
        assert_budgeted_native_mutex_wait(BudgetedMutexRoute::AdoptIngress, false)
    }

    #[derive(Clone, Copy)]
    enum OwnedScanInterruption {
        Deadline,
        LocalCancel,
        ExternalStop,
    }

    fn assert_owned_discovery_mutex_wait(
        has_work: bool,
        interruption: OwnedScanInterruption,
    ) -> TestResult {
        use std::sync::mpsc;

        let root = tempfile::tempdir()?;
        let (catalog, genesis) = setup(root.path())?;
        if has_work {
            let _published = publish_work_next(&catalog, &genesis, 1, &owner_work(92), 12004, 158)?;
        }
        let catalog = Arc::new(catalog);
        let held = catalog.lock()?;
        let deadline = Instant::now()
            .checked_add(if matches!(interruption, OwnedScanInterruption::Deadline) {
                Duration::from_millis(120)
            } else {
                Duration::from_secs(5)
            })
            .ok_or("deadline overflow")?;
        let stop = Arc::new(AtomicBool::new(false));
        let request = match interruption {
            OwnedScanInterruption::ExternalStop => {
                quanta_index_core::RequestBudgetV1::until_with_external_cancellation(
                    deadline,
                    Arc::clone(&stop),
                )
            }
            OwnedScanInterruption::Deadline | OwnedScanInterruption::LocalCancel => {
                quanta_index_core::RequestBudgetV1::until(deadline)
            }
        };
        let mut budget = WorkBudget::with_request(
            WorkLimits {
                reads: 100,
                writes: 100,
                bytes: 4096,
                mutations: 100,
            },
            request.clone(),
        );
        let (started_tx, started_rx) = mpsc::channel();
        let (finished_tx, finished_rx) = mpsc::channel();
        let worker_catalog = Arc::clone(&catalog);
        let worker = std::thread::spawn(move || {
            let _started = started_tx.send(());
            let result = worker_catalog.ready_owned_work(workspace(), 7, 1, &mut budget);
            let _finished = finished_tx.send((result, budget.first_denial()));
        });
        let started = started_rx.recv_timeout(Duration::from_secs(1));
        if started.is_err() {
            drop(held);
            worker
                .join()
                .map_err(|panic| format!("owned scan panicked: {panic:?}"))?;
            return Err("owned scan did not start".into());
        }
        std::thread::sleep(Duration::from_millis(40));
        match interruption {
            OwnedScanInterruption::Deadline => {}
            OwnedScanInterruption::LocalCancel => request.cancel_handle().cancel(),
            OwnedScanInterruption::ExternalStop => stop.store(true, Ordering::Release),
        }
        let finished = finished_rx.recv_timeout(Duration::from_secs(2));
        drop(held);
        worker
            .join()
            .map_err(|panic| format!("owned scan panicked: {panic:?}"))?;
        let (result, denial) = finished
            .map_err(|timeout| format!("owned scan waited beyond original budget: {timeout}"))?;
        match interruption {
            OwnedScanInterruption::Deadline => {
                check_eq!(result, Ok(OwnedReadyOutcome::RetryDeadline));
                check_eq!(denial, Some(WorkBudgetDenial::Deadline));
            }
            OwnedScanInterruption::LocalCancel | OwnedScanInterruption::ExternalStop => {
                check_eq!(result, Err(IndexError::Cancelled));
                check_eq!(denial, Some(WorkBudgetDenial::Cancelled));
            }
        }
        let ready = require_owned_ready(catalog.ready_owned_work(
            workspace(),
            7,
            1,
            &mut admission_budget(),
        )?)?;
        check_eq!(ready.len(), usize::from(has_work));
        Ok(())
    }

    #[test]
    fn owned_discovery_deadline_retries_with_empty_and_ready_scopes() -> TestResult {
        assert_owned_discovery_mutex_wait(false, OwnedScanInterruption::Deadline)?;
        assert_owned_discovery_mutex_wait(true, OwnedScanInterruption::Deadline)
    }

    #[test]
    fn owned_discovery_cancellation_is_not_retried() -> TestResult {
        assert_owned_discovery_mutex_wait(true, OwnedScanInterruption::LocalCancel)
    }

    #[test]
    fn owned_discovery_external_stop_interrupts_held_catalog_mutex() -> TestResult {
        assert_owned_discovery_mutex_wait(true, OwnedScanInterruption::ExternalStop)
    }

    fn assert_work_status_mutex_wait(has_work: bool, cancel: bool) -> TestResult {
        use std::sync::mpsc;

        let root = tempfile::tempdir()?;
        let (catalog, genesis) = setup(root.path())?;
        let intent = owner_work(96);
        if has_work {
            let _published = publish_work_next(&catalog, &genesis, 1, &intent, 12009, 163)?;
        }
        let catalog = Arc::new(catalog);
        let held = catalog.lock()?;
        let deadline = Instant::now()
            .checked_add(if cancel {
                Duration::from_secs(5)
            } else {
                Duration::from_millis(120)
            })
            .ok_or("deadline overflow")?;
        let request = quanta_index_core::RequestBudgetV1::until(deadline);
        let budget = WorkBudget::with_request(
            WorkLimits {
                reads: 100,
                writes: 100,
                bytes: 4096,
                mutations: 100,
            },
            request.clone(),
        );
        let (started_tx, started_rx) = mpsc::channel();
        let (finished_tx, finished_rx) = mpsc::channel();
        let worker_catalog = Arc::clone(&catalog);
        let worker = std::thread::spawn(move || {
            let _started = started_tx.send(());
            let result = worker_catalog.work_status(&intent, &budget);
            let _finished = finished_tx.send((result, budget.first_denial()));
        });
        if started_rx.recv_timeout(Duration::from_secs(1)).is_err() {
            drop(held);
            worker
                .join()
                .map_err(|panic| format!("work-status worker panicked: {panic:?}"))?;
            return Err("work-status worker did not start".into());
        }
        std::thread::sleep(Duration::from_millis(40));
        if cancel {
            request.cancel_handle().cancel();
        }
        let finished = finished_rx.recv_timeout(Duration::from_secs(2));
        drop(held);
        worker
            .join()
            .map_err(|panic| format!("work-status worker panicked: {panic:?}"))?;
        let (result, denial) = finished
            .map_err(|timeout| format!("work status waited beyond original budget: {timeout}"))?;
        check_eq!(
            result,
            Err(if cancel {
                IndexError::Cancelled
            } else {
                IndexError::ResourceLimited
            })
        );
        check_eq!(
            denial,
            Some(if cancel {
                WorkBudgetDenial::Cancelled
            } else {
                WorkBudgetDenial::Deadline
            })
        );
        check_eq!(
            catalog.work_status(&intent, &admission_budget())?,
            if has_work {
                WorkStatus::Pending
            } else {
                WorkStatus::Unknown
            }
        );
        Ok(())
    }

    #[test]
    fn work_status_cancelled_while_catalog_mutex_is_held() -> TestResult {
        assert_work_status_mutex_wait(true, true)
    }

    #[test]
    fn work_status_deadline_while_catalog_mutex_is_held() -> TestResult {
        assert_work_status_mutex_wait(false, false)
    }

    fn assert_operation_inspect_mutex_wait(cancel: bool) -> TestResult {
        use std::sync::mpsc;

        let root = tempfile::tempdir()?;
        let (catalog, _) = setup(root.path())?;
        let operation = operation(991, 201);
        let key = operation.key;
        let attempt = admit_new(&catalog, operation)?;
        let catalog = Arc::new(catalog);
        let held = catalog.lock()?;
        let deadline = Instant::now()
            .checked_add(if cancel {
                Duration::from_secs(5)
            } else {
                Duration::from_millis(120)
            })
            .ok_or("inspect deadline overflow")?;
        let request = quanta_index_core::RequestBudgetV1::until(deadline);
        let worker_request = request.clone();
        let worker_catalog = Arc::clone(&catalog);
        let (started_tx, started_rx) = mpsc::channel();
        let (finished_tx, finished_rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let budget = WorkBudget::with_request(
                WorkLimits {
                    reads: 100,
                    writes: 100,
                    bytes: 4096,
                    mutations: 100,
                },
                worker_request,
            );
            let _started = started_tx.send(());
            let result = worker_catalog.inspect(key, OperationInspectAccess::Request(&budget));
            let _finished = finished_tx.send((result, budget.first_denial()));
        });
        if started_rx.recv_timeout(Duration::from_secs(1)).is_err() {
            drop(held);
            worker
                .join()
                .map_err(|panic| format!("inspect worker panicked: {panic:?}"))?;
            return Err("inspect worker did not start".into());
        }
        std::thread::sleep(Duration::from_millis(40));
        if cancel {
            request.cancel_handle().cancel();
        }
        let finished = finished_rx.recv_timeout(Duration::from_secs(2));
        drop(held);
        worker
            .join()
            .map_err(|panic| format!("inspect worker panicked: {panic:?}"))?;
        let (result, denial) = finished
            .map_err(|timeout| format!("inspect waited beyond original budget: {timeout}"))?;
        check_eq!(
            result,
            Err(if cancel {
                IndexError::Cancelled
            } else {
                IndexError::ResourceLimited
            })
        );
        check_eq!(
            denial,
            Some(if cancel {
                WorkBudgetDenial::Cancelled
            } else {
                WorkBudgetDenial::Deadline
            })
        );
        check_eq!(
            catalog.inspect(key, OperationInspectAccess::PostFailureReconcile)?,
            OperationStatus::Pending(attempt)
        );
        check_eq!(
            inspect_with_test_budget(&catalog, key)?,
            OperationStatus::Pending(attempt)
        );
        Ok(())
    }

    #[test]
    fn operation_inspect_cancelled_while_catalog_mutex_is_held() -> TestResult {
        assert_operation_inspect_mutex_wait(true)
    }

    #[test]
    fn operation_inspect_deadline_while_catalog_mutex_is_held() -> TestResult {
        assert_operation_inspect_mutex_wait(false)
    }

    #[test]
    fn operation_inspect_expired_request_does_not_return_unknown() -> TestResult {
        let root = tempfile::tempdir()?;
        let (catalog, _) = setup(root.path())?;
        let key = operation(992, 202).key;
        let expired = WorkBudget::new(
            WorkLimits {
                reads: 100,
                writes: 100,
                bytes: 4096,
                mutations: 100,
            },
            Instant::now(),
        );
        check_eq!(
            catalog.inspect(key, OperationInspectAccess::Request(&expired)),
            Err(IndexError::ResourceLimited)
        );
        check_eq!(
            catalog.inspect(key, OperationInspectAccess::PostFailureReconcile)?,
            OperationStatus::Unknown
        );
        Ok(())
    }

    #[test]
    fn work_status_expired_budget_never_returns_pending_or_unknown() -> TestResult {
        let root = tempfile::tempdir()?;
        let (catalog, genesis) = setup(root.path())?;
        let intent = owner_work(97);
        let _published = publish_work_next(&catalog, &genesis, 1, &intent, 12010, 164)?;
        for candidate in [intent, owner_work(98)] {
            let expired = WorkBudget::new(
                WorkLimits {
                    reads: 100,
                    writes: 100,
                    bytes: 4096,
                    mutations: 100,
                },
                Instant::now(),
            );
            check_eq!(
                catalog.work_status(&candidate, &expired),
                Err(IndexError::ResourceLimited)
            );
            check_eq!(expired.first_denial(), Some(WorkBudgetDenial::Deadline));
        }
        check_eq!(
            catalog.work_status(&intent, &admission_budget())?,
            WorkStatus::Pending
        );
        check_eq!(
            catalog.work_status(&owner_work(98), &admission_budget())?,
            WorkStatus::Unknown
        );
        Ok(())
    }

    #[derive(Clone, Copy)]
    enum ManualWorkRoute {
        Claim,
        Renew,
    }

    fn manual_fence_next(catalog: &SqliteCatalog) -> Result<i64, Box<dyn Error>> {
        let guard = catalog.lock()?;
        let next = guard.query_row(
            "SELECT next FROM native_counter_v1 WHERE name='attempt_fence'",
            [],
            |row| row.get(0),
        )?;
        drop(guard);
        Ok(next)
    }

    fn assert_manual_work_mutex_wait(route: ManualWorkRoute, cancel: bool) -> TestResult {
        use std::sync::mpsc;

        let root = tempfile::tempdir()?;
        let (catalog, genesis) = setup(root.path())?;
        let intent = owner_work(99);
        let _published = publish_work_next(&catalog, &genesis, 1, &intent, 12011, 165)?;
        let existing = match route {
            ManualWorkRoute::Claim => None,
            ManualWorkRoute::Renew => {
                Some(catalog.claim_work(&intent, Duration::from_secs(5), &admission_budget())?)
            }
        };
        let before = catalog.work_status(&intent, &admission_budget())?;
        let fence_before = manual_fence_next(&catalog)?;
        let catalog = Arc::new(catalog);
        let held = catalog.lock()?;
        let deadline = Instant::now()
            .checked_add(if cancel {
                Duration::from_secs(5)
            } else {
                Duration::from_millis(120)
            })
            .ok_or("deadline overflow")?;
        let request = quanta_index_core::RequestBudgetV1::until(deadline);
        let budget = WorkBudget::with_request(
            WorkLimits {
                reads: 100,
                writes: 100,
                bytes: 4096,
                mutations: 100,
            },
            request.clone(),
        );
        let (started_tx, started_rx) = mpsc::channel();
        let (finished_tx, finished_rx) = mpsc::channel();
        let worker_catalog = Arc::clone(&catalog);
        let worker = std::thread::spawn(move || {
            let _started = started_tx.send(());
            let result = match (route, existing) {
                (ManualWorkRoute::Claim, None) => {
                    worker_catalog.claim_work(&intent, Duration::from_secs(5), &budget)
                }
                (ManualWorkRoute::Renew, Some(claim)) => {
                    worker_catalog.renew_work(&claim, Duration::from_secs(30), &budget)
                }
                (ManualWorkRoute::Claim, Some(_)) | (ManualWorkRoute::Renew, None) => Err(
                    IndexError::BackendViolation("manual test route lost claim".into()),
                ),
            };
            let _finished = finished_tx.send((result, budget.first_denial()));
        });
        if started_rx.recv_timeout(Duration::from_secs(1)).is_err() {
            drop(held);
            worker
                .join()
                .map_err(|panic| format!("manual work worker panicked: {panic:?}"))?;
            return Err("manual work worker did not start".into());
        }
        std::thread::sleep(Duration::from_millis(40));
        if cancel {
            request.cancel_handle().cancel();
        }
        let finished = finished_rx.recv_timeout(Duration::from_secs(2));
        drop(held);
        worker
            .join()
            .map_err(|panic| format!("manual work worker panicked: {panic:?}"))?;
        let (result, denial) = finished
            .map_err(|timeout| format!("manual work waited beyond original budget: {timeout}"))?;
        check_eq!(
            result,
            Err(if cancel {
                IndexError::Cancelled
            } else {
                IndexError::ResourceLimited
            })
        );
        check_eq!(
            denial,
            Some(if cancel {
                WorkBudgetDenial::Cancelled
            } else {
                WorkBudgetDenial::Deadline
            })
        );
        check_eq!(catalog.work_status(&intent, &admission_budget())?, before);
        check_eq!(manual_fence_next(&catalog)?, fence_before);
        Ok(())
    }

    #[test]
    fn manual_claim_and_renew_cancelled_while_catalog_mutex_is_held() -> TestResult {
        assert_manual_work_mutex_wait(ManualWorkRoute::Claim, true)?;
        assert_manual_work_mutex_wait(ManualWorkRoute::Renew, true)
    }

    #[test]
    fn manual_claim_and_renew_deadline_while_catalog_mutex_is_held() -> TestResult {
        assert_manual_work_mutex_wait(ManualWorkRoute::Claim, false)?;
        assert_manual_work_mutex_wait(ManualWorkRoute::Renew, false)
    }

    #[test]
    fn manual_claim_and_renew_cancel_before_commit_roll_back_fence_and_lease() -> TestResult {
        for route in [ManualWorkRoute::Claim, ManualWorkRoute::Renew] {
            let root = tempfile::tempdir()?;
            let stop = Arc::new(AtomicBool::new(false));
            let clock = Arc::new(CancelOnNextClock {
                now: 1_000,
                armed: AtomicBool::new(false),
                stop: Arc::clone(&stop),
            });
            let (catalog, genesis) = setup_with_clock(root.path(), clock.clone())?;
            let intent = owner_work(100);
            let _published = publish_work_next(&catalog, &genesis, 1, &intent, 12012, 166)?;
            let existing = match route {
                ManualWorkRoute::Claim => None,
                ManualWorkRoute::Renew => Some(catalog.claim_work(
                    &intent,
                    Duration::from_secs(5),
                    &admission_budget(),
                )?),
            };
            let before = catalog.work_status(&intent, &admission_budget())?;
            let fence_before = manual_fence_next(&catalog)?;
            let deadline = Instant::now()
                .checked_add(Duration::from_secs(5))
                .ok_or("deadline overflow")?;
            let budget = WorkBudget::with_request(
                WorkLimits {
                    reads: 100,
                    writes: 100,
                    bytes: 4096,
                    mutations: 100,
                },
                quanta_index_core::RequestBudgetV1::until_with_external_cancellation(
                    deadline,
                    Arc::clone(&stop),
                ),
            );
            clock.armed.store(true, Ordering::Release);
            let result = match (route, existing) {
                (ManualWorkRoute::Claim, None) => {
                    catalog.claim_work(&intent, Duration::from_secs(5), &budget)
                }
                (ManualWorkRoute::Renew, Some(claim)) => {
                    catalog.renew_work(&claim, Duration::from_secs(30), &budget)
                }
                (ManualWorkRoute::Claim, Some(_)) | (ManualWorkRoute::Renew, None) => {
                    return Err("manual test route lost claim".into());
                }
            };
            check_eq!(result, Err(IndexError::Cancelled));
            check_eq!(budget.first_denial(), Some(WorkBudgetDenial::Cancelled));
            check!(stop.load(Ordering::Acquire));
            check_eq!(catalog.work_status(&intent, &admission_budget())?, before);
            check_eq!(manual_fence_next(&catalog)?, fence_before);
        }
        Ok(())
    }

    #[test]
    fn manual_claim_returns_committed_grant_after_commit_hook_cancels_request() -> TestResult {
        let root = tempfile::tempdir()?;
        let (catalog, genesis) = setup(root.path())?;
        let intent = owner_work(101);
        let _published = publish_work_next(&catalog, &genesis, 1, &intent, 12013, 167)?;
        let stop = Arc::new(AtomicBool::new(false));
        {
            let connection = catalog.lock()?;
            let hook_stop = Arc::clone(&stop);
            connection.commit_hook(Some(move || {
                hook_stop.store(true, Ordering::Release);
                false
            }))?;
        }
        let deadline = Instant::now()
            .checked_add(Duration::from_secs(5))
            .ok_or("deadline overflow")?;
        let budget = WorkBudget::with_request(
            WorkLimits {
                reads: 100,
                writes: 100,
                bytes: 4096,
                mutations: 100,
            },
            quanta_index_core::RequestBudgetV1::until_with_external_cancellation(
                deadline,
                Arc::clone(&stop),
            ),
        );
        let claim = catalog.claim_work(&intent, Duration::from_secs(5), &budget)?;
        check!(stop.load(Ordering::Acquire));
        {
            let connection = catalog.lock()?;
            connection.commit_hook(None::<fn() -> bool>)?;
        }
        check_eq!(
            catalog.work_status(&intent, &admission_budget())?,
            WorkStatus::Claimed(claim)
        );
        Ok(())
    }

    fn assert_owned_acquire_mutex_wait(interruption: OwnedScanInterruption) -> TestResult {
        use std::sync::mpsc;

        let root = tempfile::tempdir()?;
        let (catalog, genesis) = setup(root.path())?;
        let intent = owner_work(93);
        let _published = publish_work_next(&catalog, &genesis, 1, &intent, 12006, 160)?;
        let catalog = Arc::new(catalog);
        let held = catalog.lock()?;
        let deadline = Instant::now()
            .checked_add(if matches!(interruption, OwnedScanInterruption::Deadline) {
                Duration::from_millis(120)
            } else {
                Duration::from_secs(5)
            })
            .ok_or("deadline overflow")?;
        let stop = Arc::new(AtomicBool::new(false));
        let request = match interruption {
            OwnedScanInterruption::Deadline => quanta_index_core::RequestBudgetV1::until(deadline),
            OwnedScanInterruption::ExternalStop => {
                quanta_index_core::RequestBudgetV1::until_with_external_cancellation(
                    deadline,
                    Arc::clone(&stop),
                )
            }
            OwnedScanInterruption::LocalCancel => {
                return Err("local cancellation is not an acquire test mode".into());
            }
        };
        let budget = WorkBudget::with_request(
            WorkLimits {
                reads: 100,
                writes: 100,
                bytes: 4096,
                mutations: 100,
            },
            request,
        );
        let (started_tx, started_rx) = mpsc::channel();
        let (finished_tx, finished_rx) = mpsc::channel();
        let worker_catalog = Arc::clone(&catalog);
        let worker = std::thread::spawn(move || {
            let _started = started_tx.send(());
            let result =
                worker_catalog.acquire_owned_work(&intent, retirement_owned_policy(), &budget);
            let _finished = finished_tx.send((result, budget.first_denial()));
        });
        if started_rx.recv_timeout(Duration::from_secs(1)).is_err() {
            drop(held);
            worker
                .join()
                .map_err(|panic| format!("owned acquire panicked: {panic:?}"))?;
            return Err("owned acquire did not start".into());
        }
        std::thread::sleep(Duration::from_millis(40));
        if matches!(interruption, OwnedScanInterruption::ExternalStop) {
            stop.store(true, Ordering::Release);
        }
        let finished = finished_rx.recv_timeout(Duration::from_secs(2));
        drop(held);
        worker
            .join()
            .map_err(|panic| format!("owned acquire panicked: {panic:?}"))?;
        let (result, denial) = finished
            .map_err(|timeout| format!("owned acquire waited beyond original budget: {timeout}"))?;
        match interruption {
            OwnedScanInterruption::Deadline => {
                check_eq!(result, Err(IndexError::ResourceLimited));
                check_eq!(denial, Some(WorkBudgetDenial::Deadline));
            }
            OwnedScanInterruption::ExternalStop => {
                check_eq!(result, Err(IndexError::Cancelled));
                check_eq!(denial, Some(WorkBudgetDenial::Cancelled));
            }
            OwnedScanInterruption::LocalCancel => {
                return Err("local cancellation is not an acquire test mode".into());
            }
        }
        check_eq!(
            catalog.work_status(&intent, &admission_budget())?,
            WorkStatus::Pending
        );
        let connection = catalog.lock()?;
        let owned: i64 =
            connection.query_row("SELECT COUNT(*) FROM native_owned_work_v1", [], |row| {
                row.get(0)
            })?;
        let requests: i64 =
            connection.query_row("SELECT COUNT(*) FROM native_owned_request_v1", [], |row| {
                row.get(0)
            })?;
        drop(connection);
        check_eq!(owned, 0);
        check_eq!(requests, 0);
        let retry =
            catalog.acquire_owned_work(&intent, retirement_owned_policy(), &admission_budget())?;
        check!(retry.is_some());
        check!(matches!(
            catalog.work_status(&intent, &admission_budget())?,
            WorkStatus::Claimed(_)
        ));
        Ok(())
    }

    #[test]
    fn owned_acquire_deadline_refuses_held_catalog_mutex_without_claim() -> TestResult {
        assert_owned_acquire_mutex_wait(OwnedScanInterruption::Deadline)
    }

    #[test]
    fn owned_acquire_external_stop_refuses_held_catalog_mutex_without_claim() -> TestResult {
        assert_owned_acquire_mutex_wait(OwnedScanInterruption::ExternalStop)
    }

    #[test]
    fn owned_acquire_cancellation_before_commit_rolls_back_claim_and_request() -> TestResult {
        let root = tempfile::tempdir()?;
        let stop = Arc::new(AtomicBool::new(false));
        let clock = Arc::new(CancelOnNextClock {
            now: 1_000,
            armed: AtomicBool::new(false),
            stop: Arc::clone(&stop),
        });
        let (catalog, genesis) = setup_with_clock(root.path(), clock.clone())?;
        let intent = owner_work(94);
        let _published = publish_work_next(&catalog, &genesis, 1, &intent, 12007, 161)?;
        let deadline = Instant::now()
            .checked_add(Duration::from_secs(5))
            .ok_or("deadline overflow")?;
        let budget = WorkBudget::with_request(
            WorkLimits {
                reads: 100,
                writes: 100,
                bytes: 4096,
                mutations: 100,
            },
            quanta_index_core::RequestBudgetV1::until_with_external_cancellation(
                deadline,
                Arc::clone(&stop),
            ),
        );
        clock.armed.store(true, Ordering::Release);
        check_eq!(
            catalog.acquire_owned_work(&intent, retirement_owned_policy(), &budget),
            Err(IndexError::Cancelled)
        );
        check_eq!(budget.first_denial(), Some(WorkBudgetDenial::Cancelled));
        check!(stop.load(Ordering::Acquire));
        check_eq!(
            catalog.work_status(&intent, &admission_budget())?,
            WorkStatus::Pending
        );
        let connection = catalog.lock()?;
        let owned: i64 =
            connection.query_row("SELECT COUNT(*) FROM native_owned_work_v1", [], |row| {
                row.get(0)
            })?;
        let requests: i64 =
            connection.query_row("SELECT COUNT(*) FROM native_owned_request_v1", [], |row| {
                row.get(0)
            })?;
        drop(connection);
        check_eq!(owned, 0);
        check_eq!(requests, 0);
        let retry =
            catalog.acquire_owned_work(&intent, retirement_owned_policy(), &admission_budget())?;
        check!(retry.is_some());
        check!(matches!(
            catalog.work_status(&intent, &admission_budget())?,
            WorkStatus::Claimed(_)
        ));
        Ok(())
    }

    #[test]
    fn owned_acquire_postcommit_cancel_recovers_same_frozen_assignment() -> TestResult {
        let root = tempfile::tempdir()?;
        let (catalog, genesis) = setup(root.path())?;
        let intent = owner_work(95);
        let _published = publish_work_next(&catalog, &genesis, 1, &intent, 12008, 162)?;
        let policy = OwnedWorkPolicy {
            total_timeout_ms: 10_000,
            lease_ms: 2_000,
            ..retirement_owned_policy()
        };
        let stop = Arc::new(AtomicBool::new(false));
        {
            let connection = catalog.lock()?;
            let hook_stop = Arc::clone(&stop);
            connection.commit_hook(Some(move || {
                hook_stop.store(true, Ordering::Release);
                false
            }))?;
        }
        let deadline = Instant::now()
            .checked_add(Duration::from_secs(5))
            .ok_or("deadline overflow")?;
        let budget = WorkBudget::with_request(
            WorkLimits {
                reads: 100,
                writes: 100,
                bytes: 4096,
                mutations: 100,
            },
            quanta_index_core::RequestBudgetV1::until_with_external_cancellation(
                deadline,
                Arc::clone(&stop),
            ),
        );
        check_eq!(
            catalog.acquire_owned_work(&intent, policy, &budget),
            Err(IndexError::Cancelled)
        );
        check_eq!(budget.first_denial(), Some(WorkBudgetDenial::Cancelled));
        check!(stop.load(Ordering::Acquire));
        let connection = catalog.lock()?;
        connection.commit_hook(None::<fn() -> bool>)?;
        let persisted = owned_work::read_owned(&connection, &intent)?
            .ok_or("postcommit cancellation lost durable assignment")?;
        let owned: i64 =
            connection.query_row("SELECT COUNT(*) FROM native_owned_work_v1", [], |row| {
                row.get(0)
            })?;
        let requests: i64 =
            connection.query_row("SELECT COUNT(*) FROM native_owned_request_v1", [], |row| {
                row.get(0)
            })?;
        drop(connection);
        check_eq!(owned, 1);
        check_eq!(requests, 1);
        check_eq!(persisted.policy, policy);
        check_eq!(persisted.dispatches, 0);
        check!(!persisted.active);
        let WorkStatus::Claimed(claim) = catalog.work_status(&intent, &admission_budget())? else {
            return Err("postcommit cancellation lost durable claim".into());
        };
        let UpdateKind::Materialize {
            claim: request_claim,
        } = &persisted.request.kind
        else {
            return Err("postcommit cancellation request kind changed".into());
        };
        check_eq!(claim, *request_claim);
        let changed_policy = OwnedWorkPolicy {
            max_dispatches: 3,
            ..policy
        };
        let recovered = catalog
            .acquire_owned_work(&intent, changed_policy, &admission_budget())?
            .ok_or("postcommit cancellation assignment was not recoverable")?;
        check_eq!(recovered, persisted);
        Ok(())
    }

    #[test]
    fn owned_discovery_read_quota_is_not_retried() -> TestResult {
        let root = tempfile::tempdir()?;
        let (catalog, genesis) = setup(root.path())?;
        let _published = publish_work_next(&catalog, &genesis, 1, &owner_work(92), 12005, 159)?;
        let mut budget = WorkBudget::with_request(
            WorkLimits {
                reads: 0,
                writes: 100,
                bytes: 4096,
                mutations: 100,
            },
            quanta_index_core::RequestBudgetV1::for_duration(Duration::from_secs(30)),
        );
        check_eq!(
            catalog.ready_owned_work(workspace(), 7, 1, &mut budget),
            Err(IndexError::ResourceLimited)
        );
        check_eq!(budget.first_denial(), Some(WorkBudgetDenial::Reads));
        Ok(())
    }

    #[test]
    fn read_settlement_release_is_exact_and_observes_last_pin_in_one_commit() -> TestResult {
        use quanta_index_core::index::{CatalogReadSettlementPort, NativeReadSettlementScope};

        let root = tempfile::tempdir()?;
        let (catalog, genesis) = setup(root.path())?;
        let next = publish_plain_next(&catalog, &genesis, 1, 191)?;
        let session = catalog.open_read_session()?;
        let first = acquire_ticket_with_test_budget(
            &catalog,
            session.as_ref(),
            ViewSelection::Exact(genesis.identity),
            Duration::from_secs(30),
        )?;
        let second = acquire_ticket_with_test_budget(
            &catalog,
            session.as_ref(),
            ViewSelection::Exact(genesis.identity),
            Duration::from_secs(30),
        )?;
        let scope = NativeReadSettlementScope {
            workspace: workspace(),
            epoch: 7,
            target: genesis.identity,
        };
        check_eq!(
            catalog.release_held_ticket(
                session.as_ref(),
                first.lease,
                NativeReadSettlementScope {
                    target: next.identity,
                    ..scope
                },
                &mut admission_budget(),
            ),
            Err(IndexError::SnapshotUnavailable)
        );
        catalog.validate_ticket(session.as_ref(), first.lease)?;
        catalog.validate_ticket(session.as_ref(), second.lease)?;
        let first_release = catalog.release_held_ticket(
            session.as_ref(),
            first.lease,
            scope,
            &mut admission_budget(),
        )?;
        check!(first_release.has_remaining_target_pins);
        let first_snapshot =
            catalog.observe_read_settlement(session.as_ref(), scope, &mut admission_budget())?;
        check!(first_snapshot.has_remaining_target_pins);
        catalog.validate_ticket(session.as_ref(), second.lease)?;
        let last_release = catalog.release_held_ticket(
            session.as_ref(),
            second.lease,
            scope,
            &mut admission_budget(),
        )?;
        check!(!last_release.has_remaining_target_pins);
        let after_last_release =
            catalog.observe_read_settlement(session.as_ref(), scope, &mut admission_budget())?;
        check!(!after_last_release.has_remaining_target_pins);
        check!(first_release.has_remaining_target_pins);
        check_eq!(
            catalog.validate_ticket(session.as_ref(), second.lease),
            Err(IndexError::SnapshotUnavailable)
        );
        Ok(())
    }

    #[test]
    fn read_settlement_requires_the_original_live_session_guard() -> TestResult {
        use quanta_index_core::index::{
            CatalogReadSettlementPort, NativeReadSettlementScope, NativeReadSettlementStatus,
            ReadSessionLease, ReadSessionRef,
        };

        struct ForeignSession(ReadSessionRef);
        impl ReadSessionLease for ForeignSession {
            fn binding(&self) -> ReadSessionRef {
                self.0
            }
        }
        let root = tempfile::tempdir()?;
        let (catalog, genesis) = setup(root.path())?;
        let session = catalog.open_read_session()?;
        let ticket = acquire_ticket_with_test_budget(
            &catalog,
            session.as_ref(),
            ViewSelection::Exact(genesis.identity),
            Duration::from_secs(30),
        )?;
        let scope = NativeReadSettlementScope {
            workspace: workspace(),
            epoch: 7,
            target: genesis.identity,
        };
        let released = catalog.release_held_ticket(
            session.as_ref(),
            ticket.lease,
            scope,
            &mut admission_budget(),
        )?;
        check!(!released.has_remaining_target_pins);
        let observed =
            catalog.observe_read_settlement(session.as_ref(), scope, &mut admission_budget())?;
        check_eq!(observed.scope, scope);
        check_eq!(observed.exact_floor, 0);
        check_eq!(observed.status, NativeReadSettlementStatus::Pending);
        check!(matches!(
            catalog.observe_read_settlement(
                &ForeignSession(session.binding()),
                scope,
                &mut admission_budget(),
            ),
            Err(IndexError::InvalidInput(_))
        ));
        Ok(())
    }

    #[test]
    fn read_settlement_queue_empty_does_not_hide_eligible_old_view() -> TestResult {
        use quanta_index_core::index::{
            CatalogReadSettlementPort, NativeReadSettlementScope, NativeReadSettlementStatus,
        };

        let root = tempfile::tempdir()?;
        let (catalog, genesis) = setup(root.path())?;
        let session = catalog.open_read_session()?;
        let ticket = acquire_ticket_with_test_budget(
            &catalog,
            session.as_ref(),
            ViewSelection::Exact(genesis.identity),
            Duration::from_secs(30),
        )?;
        let _next = publish_plain_next(&catalog, &genesis, 1, 192)?;
        check_eq!(
            catalog.advance_exact_history_floor(workspace(), 7, 1, &mut admission_budget())?,
            1
        );
        let scope = NativeReadSettlementScope {
            workspace: workspace(),
            epoch: 7,
            target: genesis.identity,
        };
        check!(
            !catalog
                .release_held_ticket(
                    session.as_ref(),
                    ticket.lease,
                    scope,
                    &mut admission_budget(),
                )?
                .has_remaining_target_pins
        );
        let queue_count: i64 =
            catalog
                .lock()?
                .query_row("SELECT COUNT(*) FROM native_gc_queue_v1", [], |row| {
                    row.get(0)
                })?;
        check_eq!(queue_count, 0);
        let observed =
            catalog.observe_read_settlement(session.as_ref(), scope, &mut admission_budget())?;
        check_eq!(observed.exact_floor, 1);
        check_eq!(observed.status, NativeReadSettlementStatus::Pending);
        Ok(())
    }

    #[test]
    fn read_settlement_budget_refusal_never_releases_or_reports_zero() -> TestResult {
        use quanta_index_core::index::{CatalogReadSettlementPort, NativeReadSettlementScope};

        let root = tempfile::tempdir()?;
        let (catalog, genesis) = setup(root.path())?;
        let session = catalog.open_read_session()?;
        let ticket = acquire_ticket_with_test_budget(
            &catalog,
            session.as_ref(),
            ViewSelection::Exact(genesis.identity),
            Duration::from_secs(30),
        )?;
        let scope = NativeReadSettlementScope {
            workspace: workspace(),
            epoch: 7,
            target: genesis.identity,
        };
        let mut denied = WorkBudget::new(
            WorkLimits {
                reads: 0,
                writes: 0,
                bytes: 0,
                mutations: 0,
            },
            Instant::now() + Duration::from_secs(30),
        );
        check_eq!(
            catalog.observe_read_settlement(session.as_ref(), scope, &mut denied),
            Err(IndexError::ResourceLimited)
        );
        check_eq!(
            catalog.release_held_ticket(session.as_ref(), ticket.lease, scope, &mut denied),
            Err(IndexError::ResourceLimited)
        );
        catalog.validate_ticket(session.as_ref(), ticket.lease)?;
        check!(
            !catalog
                .release_held_ticket(
                    session.as_ref(),
                    ticket.lease,
                    scope,
                    &mut admission_budget(),
                )?
                .has_remaining_target_pins
        );
        Ok(())
    }

    #[test]
    fn read_settlement_live_lifecycle_validator_rejects_corrupt_digest() -> TestResult {
        let (_root, catalog, _ingress) = managed_adoption_fixture()?;
        let profile = object(30, vec![]).reference;
        check!(reclaim::checked_live_object(
            &*catalog.lock()?,
            profile,
            &mut admission_budget(),
        )?);
        let changed = catalog.lock()?.execute(
            "UPDATE native_gc_object_v1 SET row_digest=zeroblob(32)
             WHERE object=?1 AND phase=0",
            params![object_key(profile)],
        )?;
        check_eq!(changed, 1);
        // The settlement scan calls this exact lifecycle validator before its
        // reference check. This fixture has not advanced the retention floor,
        // so observing settlement itself would exit Pending before that scan.
        check!(matches!(
            reclaim::checked_live_object(&*catalog.lock()?, profile, &mut admission_budget()),
            Err(IndexError::Storage(_))
        ));
        Ok(())
    }

    #[test]
    fn read_settlement_rejects_registered_object_without_lifecycle() -> TestResult {
        let (_root, catalog, _ingress) = managed_adoption_fixture()?;
        let profile = object(30, vec![]).reference;
        read_settlement::verify_object_lifecycle_coverage(
            &*catalog.lock()?,
            &mut admission_budget(),
        )?;
        let changed = catalog.lock()?.execute(
            "DELETE FROM native_gc_object_v1 WHERE object=?1 AND phase=0",
            params![object_key(profile)],
        )?;
        check_eq!(changed, 1);
        let still_registered: i64 = catalog.lock()?.query_row(
            "SELECT COUNT(*) FROM native_object_v1 WHERE object=?1",
            params![object_key(profile)],
            |row| row.get(0),
        )?;
        check_eq!(still_registered, 1);
        // This fixture is below neither retention floor nor release frontier;
        // direct coverage asserts the exact predicate used before zero status.
        check!(matches!(
            read_settlement::verify_object_lifecycle_coverage(
                &*catalog.lock()?,
                &mut admission_budget(),
            ),
            Err(IndexError::Storage(_))
        ));
        Ok(())
    }

    #[test]
    fn read_settlement_rejects_object_keys_before_the_first_blob_page() -> TestResult {
        for insert in [
            "INSERT INTO native_object_v1(object,payload,row_digest)
             VALUES (?1,x'00',zeroblob(32))",
            "INSERT INTO native_object_install_v1(object,payload,row_digest)
             VALUES (?1,x'00',zeroblob(32))",
        ] {
            for invalid_key in [
                rusqlite::types::Value::Integer(-1),
                rusqlite::types::Value::Text("invalid object key".into()),
                rusqlite::types::Value::Blob(Vec::new()),
            ] {
                let (_root, catalog, _ingress) = managed_adoption_fixture()?;
                read_settlement::verify_object_lifecycle_coverage(
                    &*catalog.lock()?,
                    &mut admission_budget(),
                )?;
                let changed = catalog.lock()?.execute(insert, params![invalid_key])?;
                check_eq!(changed, 1);
                check!(matches!(
                    read_settlement::verify_object_lifecycle_coverage(
                        &*catalog.lock()?,
                        &mut admission_budget(),
                    ),
                    Err(IndexError::Storage(_))
                ));
            }
        }
        Ok(())
    }

    #[test]
    fn read_settlement_preflights_view_bytes_and_root_shape_before_release() -> TestResult {
        use quanta_index_core::index::{CatalogReadSettlementPort, NativeReadSettlementScope};

        let root = tempfile::tempdir()?;
        let (catalog, genesis) = setup(root.path())?;
        let session = catalog.open_read_session()?;
        let ticket = acquire_ticket_with_test_budget(
            &catalog,
            session.as_ref(),
            ViewSelection::Exact(genesis.identity),
            Duration::from_secs(30),
        )?;
        let scope = NativeReadSettlementScope {
            workspace: workspace(),
            epoch: 7,
            target: genesis.identity,
        };
        let mut byte_tight = WorkBudget::new(
            WorkLimits {
                reads: 100_000,
                writes: 100_000,
                bytes: 0,
                mutations: 100_000,
            },
            Instant::now() + Duration::from_secs(30),
        );
        check_eq!(
            catalog.observe_read_settlement(session.as_ref(), scope, &mut byte_tight),
            Err(IndexError::ResourceLimited)
        );
        check_eq!(
            catalog.release_held_ticket(session.as_ref(), ticket.lease, scope, &mut byte_tight),
            Err(IndexError::ResourceLimited)
        );
        catalog.validate_ticket(session.as_ref(), ticket.lease)?;

        let changed = catalog.lock()?.execute(
            "UPDATE native_view_root_v1 SET row_digest=zeroblob(64)
             WHERE workspace=?1 AND epoch=?2 AND sequence=?3 AND object=?4",
            params![
                workspace_key(workspace())?,
                7_i64,
                0_i64,
                object_key(genesis.profile)
            ],
        )?;
        check_eq!(changed, 1);
        check!(matches!(
            catalog.release_held_ticket(
                session.as_ref(),
                ticket.lease,
                scope,
                &mut admission_budget(),
            ),
            Err(IndexError::Storage(_))
        ));
        let pins: i64 = catalog.lock()?.query_row(
            "SELECT COUNT(*) FROM native_pin_v1 WHERE ticket=?1",
            params![ticket_to_integer(ticket.lease.id)?],
            |row| row.get(0),
        )?;
        check_eq!(pins, 1);
        Ok(())
    }
    include!("authority/managed_install_trace_tests.rs");
}
