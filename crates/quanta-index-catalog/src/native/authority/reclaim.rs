//! Catalog half of bounded native object reclamation.
//! Physical mutation is exclusively the paired managed store's responsibility.

use quanta_index_core::index::{
    Digest, IndexError, MAX_RECLAIM_CANDIDATES, MAX_RECLAIM_CHILDREN, MAX_RECLAIM_DRAIN_PAGE,
    ObjectNamespaceBinding, ObjectRef, PhysicalIncarnation, ReclaimCandidate, ReclaimDrainPage,
    ReclaimKind, ReclaimPermit, ReclaimTarget, ReclaimWork, ReclaimWorkPage, RegisteredObject,
    RetiringRef, WorkBudget, WorkUsage,
};
use rusqlite::{Connection, OptionalExtension as _, Transaction, params};
use serde::{Deserialize, Serialize};

use super::{
    SqliteCatalog, begin, cached_query_row, check_digest, decode, encode, exactly_one,
    fields_digest, int, next_fence, object_install, object_key, sql, uint,
};

const LIVE: i64 = 0;
const RETIRING: i64 = 1;
const RECLAIMING: i64 = 2;
const DRAINING: i64 = 3;
const DELETED: i64 = 4;
const PROVISIONAL: i64 = 0;
const REGISTERED: i64 = 1;
const MAX_TARGET_BYTES: i64 = object_install::MAX_METADATA_BYTES_SQL + 512;

#[derive(Clone)]
struct Row {
    object: ObjectRef,
    generation: u64,
    kind: ReclaimKind,
    phase: i64,
    direct_children: u32,
    drained_children: u32,
    target: Option<ReclaimTarget>,
}

#[derive(Serialize, Deserialize)]
struct StoredTarget {
    present: bool,
    root_device: u64,
    root_inode: u64,
    objects_device: u64,
    objects_inode: u64,
    lease_device: u64,
    lease_inode: u64,
    shard_device: u64,
    shard_inode: u64,
    file_device: u64,
    file_inode: u64,
    file_changed_nanos: i128,
    file_bytes: u64,
    registration: Option<RegisteredObject>,
}

fn target_bytes(target: Option<&ReclaimTarget>) -> Result<Option<Vec<u8>>, IndexError> {
    let Some(target) = target else {
        return Ok(None);
    };
    if let ReclaimTarget::Present {
        incarnation,
        registration,
    } = target
        && (incarnation.bytes != registration.bytes
            || incarnation.shard_inode == 0
            || incarnation.inode == 0)
    {
        return Err(IndexError::InvalidInput(
            "reclaim target physical stamp mismatch".into(),
        ));
    }
    let (present, namespace, file, registration) = match target {
        ReclaimTarget::Present {
            incarnation,
            registration,
        } => (
            true,
            incarnation.namespace,
            Some(incarnation),
            Some(object_install::checked_object(registration)?),
        ),
        ReclaimTarget::AbsentProvisional(namespace) => (false, *namespace, None, None),
    };
    let saved = StoredTarget {
        present,
        root_device: namespace.root_device,
        root_inode: namespace.root_inode,
        objects_device: namespace.objects_device,
        objects_inode: namespace.objects_inode,
        lease_device: namespace.lease_device,
        lease_inode: namespace.lease_inode,
        shard_device: file.map_or(0, |file| file.shard_device),
        shard_inode: file.map_or(0, |file| file.shard_inode),
        file_device: file.map_or(0, |file| file.device),
        file_inode: file.map_or(0, |file| file.inode),
        file_changed_nanos: file.map_or(0, |file| file.changed_nanos),
        file_bytes: file.map_or(0, |file| file.bytes),
        registration,
    };
    let encoded = encode(&saved)?;
    if encoded.is_empty()
        || encoded.len()
            > usize::try_from(MAX_TARGET_BYTES).map_err(|_error| IndexError::ResourceLimited)?
    {
        return Err(IndexError::ResourceLimited);
    }
    Ok(Some(encoded))
}

fn decode_target(payload: &[u8]) -> Result<ReclaimTarget, IndexError> {
    if payload.is_empty()
        || payload.len()
            > usize::try_from(MAX_TARGET_BYTES).map_err(|_error| IndexError::ResourceLimited)?
    {
        return Err(IndexError::Storage("reclaim target size invalid".into()));
    }
    let saved: StoredTarget = decode(payload)?;
    let namespace = ObjectNamespaceBinding {
        root_device: saved.root_device,
        root_inode: saved.root_inode,
        objects_device: saved.objects_device,
        objects_inode: saved.objects_inode,
        lease_device: saved.lease_device,
        lease_inode: saved.lease_inode,
    };
    if saved.present {
        if saved.shard_inode == 0 || saved.file_inode == 0 || saved.file_bytes == 0 {
            return Err(IndexError::Storage(
                "reclaim physical stamp is invalid".into(),
            ));
        }
        let registration = saved
            .registration
            .ok_or_else(|| IndexError::Storage("reclaim target lacks registration".into()))?;
        let canonical = object_install::checked_object(&registration)
            .map_err(|_error| IndexError::Storage("reclaim registration shape invalid".into()))?;
        if canonical != registration {
            return Err(IndexError::Storage(
                "reclaim target registration mismatch".into(),
            ));
        }
        if registration.bytes != saved.file_bytes {
            return Err(IndexError::Storage(
                "reclaim target registration mismatch".into(),
            ));
        }
        Ok(ReclaimTarget::Present {
            incarnation: PhysicalIncarnation {
                namespace,
                shard_device: saved.shard_device,
                shard_inode: saved.shard_inode,
                device: saved.file_device,
                inode: saved.file_inode,
                changed_nanos: saved.file_changed_nanos,
                bytes: saved.file_bytes,
            },
            registration,
        })
    } else if saved.shard_device == 0
        && saved.shard_inode == 0
        && saved.file_device == 0
        && saved.file_inode == 0
        && saved.file_changed_nanos == 0
        && saved.file_bytes == 0
        && saved.registration.is_none()
    {
        Ok(ReclaimTarget::AbsentProvisional(namespace))
    } else {
        Err(IndexError::Storage(
            "absent reclaim target has file stamp".into(),
        ))
    }
}

fn kind_value(kind: ReclaimKind) -> i64 {
    match kind {
        ReclaimKind::Provisional => PROVISIONAL,
        ReclaimKind::Registered => REGISTERED,
    }
}

fn decode_kind(kind: i64) -> Result<ReclaimKind, IndexError> {
    match kind {
        PROVISIONAL => Ok(ReclaimKind::Provisional),
        REGISTERED => Ok(ReclaimKind::Registered),
        _ => Err(IndexError::Storage("invalid reclaim object kind".into())),
    }
}

fn row_digest(row: &Row, target: Option<&Vec<u8>>) -> Result<Vec<u8>, IndexError> {
    fields_digest(
        b"qidx.native.gc.object.v1",
        &(
            object_key(row.object),
            int(row.generation)?,
            kind_value(row.kind),
            row.phase,
            i64::from(row.direct_children),
            i64::from(row.drained_children),
            target,
        ),
    )
}

fn read_row(connection: &Connection, object: ObjectRef) -> Result<Option<Row>, IndexError> {
    read_row_with_access(connection, object, RowReadAccess::Inspect)
}

#[derive(Clone, Copy)]
enum RowReadAccess {
    Inspect,
    Mutable,
}

// Persisted SQL fields feed the same canonical row/integrity validator.
type StoredLifecycleRow = (i64, i64, i64, i64, i64, Option<Vec<u8>>, Vec<u8>);

const MUTABLE_ROW_SQL: &str = "SELECT
    CASE WHEN typeof(generation)='integer' THEN generation END,
    CASE WHEN typeof(kind)='integer' THEN kind END,
    CASE WHEN typeof(phase)='integer' THEN phase END,
    CASE WHEN typeof(direct_children)='integer' THEN direct_children END,
    CASE WHEN typeof(drained_children)='integer' THEN drained_children END,
    length(target),
    CASE WHEN phase IN (0,1,4) AND length(target) IS NULL
      AND typeof(row_digest)='blob' AND length(row_digest)=32 THEN row_digest END
    FROM native_gc_object_v1 WHERE object=?1";

fn decode_mutable_row(record: &rusqlite::Row<'_>) -> Result<StoredLifecycleRow, IndexError> {
    let phase: i64 = record.get(2).map_err(|error| sql(&error))?;
    // Refuse frozen ownership before accessing any target or digest body.
    match phase {
        RECLAIMING | DRAINING => return Err(IndexError::Conflict),
        LIVE | RETIRING | DELETED => {}
        _ => return Err(IndexError::Storage("invalid reclaim phase".into())),
    }
    let target_present: Option<i64> = record.get(5).map_err(|error| sql(&error))?;
    if target_present.is_some() {
        return Err(IndexError::Storage(
            "mutable reclaim phase carries target".into(),
        ));
    }
    let checksum: Option<Vec<u8>> = record.get(6).map_err(|error| sql(&error))?;
    let checksum =
        checksum.ok_or_else(|| IndexError::Storage("reclaim object row shape mismatch".into()))?;
    Ok((
        record.get(0).map_err(|error| sql(&error))?,
        record.get(1).map_err(|error| sql(&error))?,
        phase,
        record.get(3).map_err(|error| sql(&error))?,
        record.get(4).map_err(|error| sql(&error))?,
        None,
        checksum,
    ))
}

fn read_row_with_access(
    connection: &Connection,
    object: ObjectRef,
    access: RowReadAccess,
) -> Result<Option<Row>, IndexError> {
    let key = object_key(object);
    let (target_len, stored) = match access {
        RowReadAccess::Mutable => {
            let Some(stored) = connection
                .prepare_cached(MUTABLE_ROW_SQL)
                .map_err(|error| sql(&error))?
                .query_row(params![&key], |record| Ok(decode_mutable_row(record)))
                .optional()
                .map_err(|error| sql(&error))?
                .transpose()?
            else {
                return Ok(None);
            };
            (None, stored)
        }
        RowReadAccess::Inspect => {
            let lengths: Option<(i64, Option<i64>, Option<i64>)> = connection
                .query_row(
                    "SELECT phase,length(target),length(row_digest) FROM native_gc_object_v1 WHERE object=?1",
                    params![&key],
                    |record| Ok((record.get(0)?, record.get(1)?, record.get(2)?)),
                ).optional().map_err(|error| sql(&error))?;
            let Some((_observed_phase, target_len, digest_len)) = lengths else {
                return Ok(None);
            };
            if target_len.is_some_and(|len| !(1..=MAX_TARGET_BYTES).contains(&len))
                || digest_len != Some(32)
            {
                return Err(IndexError::Storage(
                    "reclaim object row shape mismatch".into(),
                ));
            }
            let stored: StoredLifecycleRow = connection.query_row(
                "SELECT generation,kind,phase,direct_children,drained_children,target,row_digest FROM native_gc_object_v1 WHERE object=?1",
                params![&key],
                |record| Ok((record.get(0)?,record.get(1)?,record.get(2)?,record.get(3)?,record.get(4)?,record.get(5)?,record.get(6)?)),
            ).map_err(|error| sql(&error))?;
            (target_len, stored)
        }
    };
    let (generation, kind, phase, children, drained, target, checksum) = stored;
    if target
        .as_ref()
        .map(|value| i64::try_from(value.len()).map_err(|_error| IndexError::ResourceLimited))
        .transpose()?
        != target_len
        || checksum.len() != 32
        || !(LIVE..=DELETED).contains(&phase)
        || !(0..=4096_i64).contains(&children)
        || !(0..=children).contains(&drained)
        || (phase != DRAINING && drained != 0)
        || (matches!(phase, LIVE | DELETED) && children != 0)
    {
        return Err(IndexError::Storage("reclaim object row changed".into()));
    }
    let decoded_target = target.as_deref().map(decode_target).transpose()?;
    if let Some(ReclaimTarget::Present { registration, .. }) = &decoded_target {
        let mut distinct_children = registration.children.clone();
        distinct_children.sort_unstable();
        distinct_children.dedup();
        if registration.reference != object
            || distinct_children.len()
                != usize::try_from(children).map_err(|_error| IndexError::ResourceLimited)?
        {
            return Err(IndexError::Storage(
                "reclaim target differs from frozen row".into(),
            ));
        }
    }
    let row = Row {
        object,
        generation: uint(generation)?,
        kind: decode_kind(kind)?,
        phase,
        direct_children: u32::try_from(children).map_err(|_error| IndexError::ResourceLimited)?,
        drained_children: u32::try_from(drained).map_err(|_error| IndexError::ResourceLimited)?,
        target: decoded_target,
    };
    if row.generation == 0
        || checksum != row_digest(&row, target.as_ref())?
        || matches!(phase, LIVE | RETIRING | DELETED) != target.is_none()
        || matches!(phase, RECLAIMING | DRAINING) != target.is_some()
    {
        return Err(IndexError::Storage(
            "reclaim object row digest or phase mismatch".into(),
        ));
    }
    Ok(Some(row))
}

fn mutable_live_row(connection: &Connection, object: ObjectRef) -> Result<Option<Row>, IndexError> {
    read_row_with_access(connection, object, RowReadAccess::Mutable)
}

fn charged_row(
    connection: &Connection,
    object: ObjectRef,
    budget: &mut WorkBudget,
) -> Result<Option<Row>, IndexError> {
    budget.charge(WorkUsage {
        reads: 1,
        ..WorkUsage::default()
    })?;
    let lengths: Option<(Option<i64>, i64)> = connection
        .query_row(
            "SELECT length(target),length(row_digest) FROM native_gc_object_v1 WHERE object=?1",
            params![object_key(object)],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|error| sql(&error))?;
    let Some((target_length, digest_length)) = lengths else {
        return Ok(None);
    };
    if target_length.is_some_and(|length| !(1..=MAX_TARGET_BYTES).contains(&length))
        || digest_length != 32
    {
        return Err(IndexError::Storage(
            "reclaim object row shape mismatch".into(),
        ));
    }
    let bytes = target_length
        .map(uint)
        .transpose()?
        .unwrap_or(0)
        .checked_add(32)
        .ok_or(IndexError::ResourceLimited)?;
    budget.charge(WorkUsage {
        reads: 2,
        bytes,
        ..WorkUsage::default()
    })?;
    read_row(connection, object)
}

fn insert_row(connection: &Connection, row: &Row) -> Result<(), IndexError> {
    let target = target_bytes(row.target.as_ref())?;
    let changed = connection.execute(
        "INSERT INTO native_gc_object_v1(object,generation,kind,phase,direct_children,drained_children,target,row_digest) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
        params![object_key(row.object),int(row.generation)?,kind_value(row.kind),row.phase,i64::from(row.direct_children),i64::from(row.drained_children),&target,row_digest(row,target.as_ref())?],
    ).map_err(|error| sql(&error))?;
    exactly_one(changed, "insert reclaim object")
}

fn update_row(connection: &Connection, row: &Row) -> Result<(), IndexError> {
    let target = target_bytes(row.target.as_ref())?;
    let changed = connection.execute(
        "UPDATE native_gc_object_v1 SET generation=?2,kind=?3,phase=?4,direct_children=?5,drained_children=?6,target=?7,row_digest=?8 WHERE object=?1",
        params![object_key(row.object),int(row.generation)?,kind_value(row.kind),row.phase,i64::from(row.direct_children),i64::from(row.drained_children),&target,row_digest(row,target.as_ref())?],
    ).map_err(|error| sql(&error))?;
    exactly_one(changed, "update reclaim object")
}

fn queued(connection: &Connection, object: ObjectRef) -> Result<bool, IndexError> {
    let key = object_key(object);
    let checksum: Option<Option<Vec<u8>>> = connection.query_row(
        "SELECT CASE WHEN length(row_digest)=32 THEN row_digest END FROM native_gc_queue_v1 WHERE object=?1",
        params![&key], |record| record.get(0),
    ).optional().map_err(|error| sql(&error))?;
    match checksum {
        None => Ok(false),
        Some(Some(checksum)) if checksum == fields_digest(b"qidx.native.gc.queue.v1", &key)? => {
            Ok(true)
        }
        Some(_) => Err(IndexError::Storage("reclaim queue digest mismatch".into())),
    }
}

fn dequeue(connection: &Connection, object: ObjectRef) -> Result<(), IndexError> {
    if queued(connection, object)? {
        let _deleted = connection
            .execute(
                "DELETE FROM native_gc_queue_v1 WHERE object=?1",
                params![object_key(object)],
            )
            .map_err(|error| sql(&error))?;
    }
    Ok(())
}

pub(super) fn enqueue(connection: &Connection, object: ObjectRef) -> Result<bool, IndexError> {
    let Some(row) = mutable_live_row(connection, object)? else {
        return Ok(false);
    };
    enqueue_row(connection, &row)
}

fn enqueue_row(connection: &Connection, row: &Row) -> Result<bool, IndexError> {
    if row.phase == DELETED {
        return Err(IndexError::Storage(
            "deleted object still has custody".into(),
        ));
    }
    if queued(connection, row.object)? {
        return Ok(false);
    }
    let key = object_key(row.object);
    let changed = connection
        .execute(
            "INSERT INTO native_gc_queue_v1(object,row_digest) VALUES (?1,?2)",
            params![&key, fields_digest(b"qidx.native.gc.queue.v1", &key)?],
        )
        .map_err(|error| sql(&error))?;
    exactly_one(changed, "enqueue reclaim object")?;
    Ok(true)
}

/// Called in the same transaction that creates a scoped provisional intent.
pub(super) fn reserve_object(connection: &Connection, object: ObjectRef) -> Result<(), IndexError> {
    reserve_row(connection, object).map(|_row| ())
}

fn reserve_row(connection: &Connection, object: ObjectRef) -> Result<Row, IndexError> {
    reserve_row_with_fence(connection, object, || next_fence(connection))
}

fn reserve_row_with_fence(
    connection: &Connection,
    object: ObjectRef,
    allocate: impl FnOnce() -> Result<u64, IndexError>,
) -> Result<Row, IndexError> {
    let Some(mut row) = mutable_live_row(connection, object)? else {
        let row = Row {
            object,
            generation: allocate()?,
            kind: ReclaimKind::Provisional,
            phase: LIVE,
            direct_children: 0,
            drained_children: 0,
            target: None,
        };
        insert_row(connection, &row)?;
        return Ok(row);
    };
    match row.phase {
        LIVE => Ok(row),
        RETIRING | DELETED => {
            let was_deleted = row.phase == DELETED;
            row.generation = allocate()?;
            row.phase = LIVE;
            row.direct_children = 0;
            row.drained_children = 0;
            row.target = None;
            if was_deleted {
                row.kind = ReclaimKind::Provisional;
            }
            update_row(connection, &row)?;
            dequeue(connection, object)?;
            Ok(row)
        }
        RECLAIMING | DRAINING => Err(IndexError::Conflict),
        _ => Err(IndexError::Storage("invalid reclaim phase".into())),
    }
}

/// Normalize and queue one object with one authenticated lifecycle read.
pub(super) fn reserve_and_enqueue(
    transaction: &Transaction<'_>,
    object: ObjectRef,
) -> Result<bool, IndexError> {
    let row = reserve_row(transaction, object)?;
    enqueue_row(transaction, &row)
}

/// Reserve through the exact transaction's conditional global counter stream.
pub(super) fn reserve_and_enqueue_sequenced(
    object: ObjectRef,
    fences: &mut super::fence_counter::ReservationSequence<'_, '_>,
    budget: &mut WorkBudget,
) -> Result<bool, IndexError> {
    fences.ensure_open()?;
    let transaction = fences.transaction();
    let result = (|| {
        let mut allocated = false;
        let row = reserve_row_with_fence(transaction, object, || {
            allocated = true;
            fences.next(budget)
        })?;
        if !allocated {
            // Live rows did not eliminate a counter read: retain their old envelope.
            budget.charge(WorkUsage {
                reads: 1,
                ..WorkUsage::default()
            })?;
        }
        enqueue_row(transaction, &row)
    })();
    if result.is_err() {
        fences.refuse();
    }
    result
}

pub(super) fn adopt_if_known(connection: &Connection, object: ObjectRef) -> Result<(), IndexError> {
    if !is_bound(connection)? {
        return Ok(());
    }
    match mutable_live_row(connection, object)? {
        None => Ok(()),
        Some(row) if row.phase == DELETED => Ok(()),
        Some(_) => adopt(connection, object),
    }
}

pub(super) fn adopt(connection: &Connection, object: ObjectRef) -> Result<(), IndexError> {
    adopt_with_queue(connection, object, false)
}

/// An earlier child in this same confirmation packet is enqueued again before
/// commit. Authenticate its queue row without deleting and reinserting it.
///
/// Only the confirmation preflight may use this path: rollback covers failure,
/// and the subsequent confirm-and-enqueue restores an absent row atomically.
pub(super) fn adopt_confirming_input(
    connection: &Transaction<'_>,
    object: ObjectRef,
) -> Result<(), IndexError> {
    adopt_with_queue(connection, object, true)
}

fn adopt_with_queue(
    connection: &Connection,
    object: ObjectRef,
    retain_queue: bool,
) -> Result<(), IndexError> {
    if !is_bound(connection)? {
        return Ok(());
    }
    let mut row = mutable_live_row(connection, object)?
        .ok_or_else(|| IndexError::Storage("managed root lacks object lifecycle".into()))?;
    match row.phase {
        LIVE => {
            if retain_queue {
                let _present = queued(connection, object)?;
            } else {
                dequeue(connection, object)?;
            }
            Ok(())
        }
        RETIRING => {
            row.generation = next_fence(connection)?;
            row.phase = LIVE;
            row.direct_children = 0;
            row.drained_children = 0;
            update_row(connection, &row)?;
            if retain_queue {
                let _present = queued(connection, object)?;
                Ok(())
            } else {
                dequeue(connection, object)
            }
        }
        RECLAIMING | DRAINING | DELETED => Err(IndexError::Conflict),
        _ => Err(IndexError::Storage("invalid reclaim phase".into())),
    }
}

pub(super) fn from_key(key: &[u8]) -> Result<ObjectRef, IndexError> {
    let exact: [u8; 32] = key
        .try_into()
        .map_err(|_error| IndexError::Storage("object custody key width invalid".into()))?;
    Ok(ObjectRef(Digest(exact)))
}

pub(super) fn release_key(connection: &Connection, key: &[u8]) -> Result<(), IndexError> {
    release(connection, from_key(key)?)
}

pub(super) fn release(connection: &Connection, object: ObjectRef) -> Result<(), IndexError> {
    if is_bound(connection)? {
        let _queued = enqueue(connection, object)?;
    }
    Ok(())
}

pub(super) fn confirm_object(connection: &Connection, object: ObjectRef) -> Result<(), IndexError> {
    let mut row = mutable_live_row(connection, object)?
        .ok_or_else(|| IndexError::Storage("confirmed object has no lifecycle".into()))?;
    if row.phase != LIVE {
        return Err(IndexError::Conflict);
    }
    if row.kind != ReclaimKind::Registered {
        row.kind = ReclaimKind::Registered;
        update_row(connection, &row)?;
    }
    Ok(())
}

/// Confirm and queue under the caller's transaction using one authenticated row.
/// No callback or other mutation separates the update from the queue check.
pub(super) fn confirm_and_enqueue(
    connection: &Transaction<'_>,
    object: ObjectRef,
) -> Result<bool, IndexError> {
    let mut row = mutable_live_row(connection, object)?
        .ok_or_else(|| IndexError::Storage("confirmed object has no lifecycle".into()))?;
    if row.phase != LIVE {
        return Err(IndexError::Conflict);
    }
    if row.kind != ReclaimKind::Registered {
        row.kind = ReclaimKind::Registered;
        update_row(connection, &row)?;
    }
    enqueue_row(connection, &row)
}

/// Reuse the lifecycle owner's shape/digest validation for observation.
pub(super) fn checked_live_object(
    connection: &Connection,
    object: ObjectRef,
    budget: &mut WorkBudget,
) -> Result<bool, IndexError> {
    budget.charge(WorkUsage {
        reads: 2,
        bytes: 32,
        ..WorkUsage::default()
    })?;
    let row = read_row(connection, object)?
        .ok_or_else(|| IndexError::Storage("observed lifecycle row disappeared".into()))?;
    Ok(row.phase == LIVE)
}

pub(super) fn reference_exists(
    connection: &Connection,
    object: ObjectRef,
    budget: &mut WorkBudget,
) -> Result<bool, IndexError> {
    let key = object_key(object);
    for query in [
        "SELECT EXISTS(SELECT 1 FROM native_view_root_v1 WHERE object=?1)",
        "SELECT EXISTS(SELECT 1 FROM native_operation_object_v1 WHERE object=?1)",
        "SELECT EXISTS(SELECT 1 FROM native_ingress_intent_v1 WHERE object=?1)",
        "SELECT EXISTS(SELECT 1 FROM native_ingress_object_v1 WHERE object=?1)",
        "SELECT EXISTS(SELECT 1 FROM native_attempt_object_v1 WHERE object=?1)",
        "SELECT EXISTS(SELECT 1 FROM native_object_intent_v1 WHERE object=?1)",
        "SELECT EXISTS(SELECT 1 FROM native_work_object_v1 WHERE object=?1)",
        "SELECT EXISTS(SELECT 1 FROM native_bootstrap_object_v1 WHERE object=?1)",
        "SELECT EXISTS(SELECT 1 FROM native_bootstrap_v1 WHERE profile=?1)",
        "SELECT EXISTS(SELECT 1 FROM native_object_edge_v1 WHERE child=?1)",
        "SELECT EXISTS(SELECT 1 FROM native_object_install_edge_v1 WHERE child=?1)",
    ] {
        budget.charge(WorkUsage {
            reads: 1,
            ..WorkUsage::default()
        })?;
        let exists: i64 = connection
            .query_row(query, params![&key], |record| record.get(0))
            .map_err(|error| sql(&error))?;
        if exists != 0 {
            return Ok(true);
        }
    }
    super::boot_manifest::refuse_selected_orphan(connection, object, budget)?;
    Ok(false)
}

fn count_children(
    connection: &Connection,
    row: &Row,
    budget: &mut WorkBudget,
) -> Result<u32, IndexError> {
    let registration = metadata(connection, row, budget)?;
    let mut children = registration.children.clone();
    children.sort_unstable();
    children.dedup();
    if children.len() > MAX_RECLAIM_CHILDREN {
        return Err(IndexError::Storage(
            "reclaim child count exceeds wire bound".into(),
        ));
    }
    let table = match row.kind {
        ReclaimKind::Registered => "native_object_edge_v1",
        ReclaimKind::Provisional => "native_object_install_edge_v1",
    };
    let key = object_key(row.object);
    let reads = u64::try_from(children.len())
        .map_err(|_error| IndexError::ResourceLimited)?
        .checked_add(1)
        .ok_or(IndexError::ResourceLimited)?;
    budget.charge(WorkUsage {
        reads,
        bytes: reads.checked_mul(64).ok_or(IndexError::ResourceLimited)?,
        ..WorkUsage::default()
    })?;
    let query = format!(
        "SELECT CASE WHEN length(child)=32 THEN child END, CASE WHEN length(row_digest)=32 THEN row_digest END FROM {table} WHERE parent=?1 ORDER BY child LIMIT 4097"
    );
    let mut statement = connection.prepare(&query).map_err(|error| sql(&error))?;
    let mut records = statement
        .query(params![&key])
        .map_err(|error| sql(&error))?;
    for expected in &children {
        let record = records
            .next()
            .map_err(|error| sql(&error))?
            .ok_or_else(|| IndexError::Storage("reclaim direct edge missing".into()))?;
        let child: Option<Vec<u8>> = record.get(0).map_err(|error| sql(&error))?;
        let checksum: Option<Vec<u8>> = record.get(1).map_err(|error| sql(&error))?;
        let child =
            child.ok_or_else(|| IndexError::Storage("reclaim direct edge shape invalid".into()))?;
        let checksum = checksum.ok_or_else(|| {
            IndexError::Storage("reclaim direct edge checksum shape invalid".into())
        })?;
        let domain = if row.kind == ReclaimKind::Registered {
            b"qidx.native.edge.v1".as_slice()
        } else {
            b"qidx.native.installedge.v1".as_slice()
        };
        if child != object_key(*expected) || checksum != fields_digest(domain, &(&key, &child))? {
            return Err(IndexError::Storage(
                "reclaim direct edge differs from registration".into(),
            ));
        }
    }
    if records.next().map_err(|error| sql(&error))?.is_some() {
        return Err(IndexError::Storage(
            "reclaim direct edge count exceeds registration".into(),
        ));
    }
    u32::try_from(children.len()).map_err(|_error| IndexError::ResourceLimited)
}

fn candidate(row: &Row) -> ReclaimCandidate {
    ReclaimCandidate {
        object: row.object,
        generation: row.generation,
        kind: row.kind,
    }
}

fn permit(row: &Row) -> Result<ReclaimPermit, IndexError> {
    Ok(ReclaimPermit {
        retiring: RetiringRef {
            candidate: candidate(row),
            direct_children: row.direct_children,
        },
        target: row
            .target
            .clone()
            .ok_or_else(|| IndexError::Storage("reclaim permit lacks target".into()))?,
    })
}

pub(super) fn work_page(
    catalog: &SqliteCatalog,
    after: Option<ObjectRef>,
    limit: u32,
    budget: &mut WorkBudget,
) -> Result<ReclaimWorkPage, IndexError> {
    if !(1..=MAX_RECLAIM_CANDIDATES).contains(&limit) {
        return Err(IndexError::InvalidInput(
            "reclaim page limit out of bounds".into(),
        ));
    }
    budget.checkpoint()?;
    let guard = catalog.lock_native_budget(budget)?;
    let _binding = bound_namespace(&guard)?;
    let after = after.map(object_key).unwrap_or_default();
    let take = limit.checked_add(1).ok_or(IndexError::ResourceLimited)?;
    budget.charge(WorkUsage {
        reads: u64::from(take),
        bytes: u64::from(take)
            .checked_mul(64)
            .ok_or(IndexError::ResourceLimited)?,
        ..WorkUsage::default()
    })?;
    let mut statement = guard
        .prepare(
            "SELECT CASE WHEN length(object)=32 THEN object END,
                CASE WHEN length(row_digest)=32 THEN row_digest END
         FROM native_gc_queue_v1 WHERE object>?1 ORDER BY object LIMIT ?2",
        )
        .map_err(|error| sql(&error))?;
    let rows = statement
        .query_map(params![&after, i64::from(take)], |record| {
            Ok((
                record.get::<_, Option<Vec<u8>>>(0)?,
                record.get::<_, Option<Vec<u8>>>(1)?,
            ))
        })
        .map_err(|error| sql(&error))?;
    let mut items = Vec::new();
    let mut has_more = false;
    for row in rows {
        let (key, checksum) = row.map_err(|error| sql(&error))?;
        let key =
            key.ok_or_else(|| IndexError::Storage("reclaim queue key shape invalid".into()))?;
        let checksum = checksum
            .ok_or_else(|| IndexError::Storage("reclaim queue digest shape invalid".into()))?;
        if checksum != fields_digest(b"qidx.native.gc.queue.v1", &key)? {
            return Err(IndexError::Storage("reclaim queue row invalid".into()));
        }
        let object = ObjectRef(Digest(
            <[u8; 32]>::try_from(key.as_slice())
                .map_err(|_error| IndexError::Storage("reclaim queue key invalid".into()))?,
        ));
        if items.len() == usize::try_from(limit).map_err(|_error| IndexError::ResourceLimited)? {
            has_more = true;
            break;
        }
        let lifecycle = charged_row(&guard, object, budget)?
            .ok_or_else(|| IndexError::Storage("reclaim queue lacks lifecycle".into()))?;
        let work = match lifecycle.phase {
            LIVE | RETIRING => ReclaimWork::Candidate(candidate(&lifecycle)),
            RECLAIMING => ReclaimWork::Reclaiming(permit(&lifecycle)?),
            DRAINING => ReclaimWork::Draining(permit(&lifecycle)?),
            _ => {
                return Err(IndexError::Storage(
                    "deleted reclaim object remains queued".into(),
                ));
            }
        };
        items.push(work);
    }
    drop(statement);
    drop(guard);
    Ok(ReclaimWorkPage { items, has_more })
}

pub(super) fn retire(
    catalog: &SqliteCatalog,
    requested: ReclaimCandidate,
    budget: &mut WorkBudget,
) -> Result<Option<RetiringRef>, IndexError> {
    budget.checkpoint()?;
    let mut guard = catalog.lock_native_budget(budget)?;
    let tx = begin(&mut guard)?;
    let Some(mut row) = charged_row(&tx, requested.object, budget)? else {
        return Ok(None);
    };
    if candidate(&row) != requested
        || !matches!(row.phase, LIVE | RETIRING)
        || !queued(&tx, requested.object)?
    {
        return Ok(None);
    }
    if reference_exists(&tx, requested.object, budget)? {
        let mutations = if row.phase == RETIRING { 3 } else { 1 };
        budget.charge(WorkUsage {
            writes: mutations,
            mutations,
            ..WorkUsage::default()
        })?;
        if row.phase == RETIRING {
            row.generation = next_fence(&tx)?;
            row.phase = LIVE;
            row.direct_children = 0;
            row.drained_children = 0;
            update_row(&tx, &row)?;
        }
        dequeue(&tx, requested.object)?;
        budget.checkpoint()?;
        tx.commit().map_err(|error| sql(&error))?;
        drop(guard);
        return Ok(None);
    }
    if row.phase == LIVE {
        row.direct_children = count_children(&tx, &row, budget)?;
        row.drained_children = 0;
        row.phase = RETIRING;
        budget.charge(WorkUsage {
            writes: 1,
            mutations: 1,
            ..WorkUsage::default()
        })?;
        update_row(&tx, &row)?;
    }
    budget.checkpoint()?;
    tx.commit().map_err(|error| sql(&error))?;
    drop(guard);
    Ok(Some(RetiringRef {
        candidate: requested,
        direct_children: row.direct_children,
    }))
}

pub(super) fn is_bound(connection: &Connection) -> Result<bool, IndexError> {
    let count: i64 = connection
        .query_row("SELECT COUNT(*) FROM native_gc_namespace_v1", [], |row| {
            row.get(0)
        })
        .map_err(|error| sql(&error))?;
    if !(0..=1).contains(&count) {
        return Err(IndexError::Storage(
            "managed namespace binding count invalid".into(),
        ));
    }
    if count == 1 {
        let _binding = bound_namespace(connection)?;
    }
    Ok(count == 1)
}

pub(super) fn require_bound(connection: &Connection) -> Result<(), IndexError> {
    bound_namespace(connection).map(|_| ())
}

pub(super) fn require_unbound(connection: &Connection) -> Result<(), IndexError> {
    if is_bound(connection)? {
        Err(IndexError::BackendViolation(
            "budgetless object callback on managed catalog".into(),
        ))
    } else {
        Ok(())
    }
}

type StoredNamespaceRow = (i64, Vec<u8>, Vec<u8>);

pub(super) fn bound_namespace(
    connection: &Connection,
) -> Result<ObjectNamespaceBinding, IndexError> {
    // The CASE arms keep oversized bodies out of Rust while one statement
    // authenticates the shape and the exact bytes from the same row snapshot.
    let row: Option<Result<StoredNamespaceRow, IndexError>> = cached_query_row(
        connection,
        "SELECT length(payload),length(row_digest),
                CASE WHEN length(payload) BETWEEN 1 AND 256 AND length(row_digest)=32 THEN payload END,
                CASE WHEN length(payload) BETWEEN 1 AND 256 AND length(row_digest)=32 THEN row_digest END
         FROM native_gc_namespace_v1 WHERE singleton=1",
        [],
        |record| {
            let size: i64 = record.get(0)?;
            let checksum_len: i64 = record.get(1)?;
            if !(1..=256).contains(&size) || checksum_len != 32 {
                return Ok(Err(IndexError::Storage(
                    "managed namespace binding shape mismatch".into(),
                )));
            }
            Ok(Ok((size, record.get(2)?, record.get(3)?)))
        },
    )
    .optional()
    .map_err(|error| sql(&error))?;
    let Some(row) = row else {
        return Err(IndexError::BackendViolation(
            "managed namespace is unbound".into(),
        ));
    };
    let (size, payload, checksum) = row?;
    if payload.len() != usize::try_from(size).map_err(|_error| IndexError::ResourceLimited)? {
        return Err(IndexError::Storage(
            "managed namespace binding changed".into(),
        ));
    }
    check_digest(b"qidx.native.gc.namespace.v1", &payload, &checksum)?;
    let values: (u64, u64, u64, u64, u64, u64) = decode(&payload)?;
    Ok(ObjectNamespaceBinding {
        root_device: values.0,
        root_inode: values.1,
        objects_device: values.2,
        objects_inode: values.3,
        lease_device: values.4,
        lease_inode: values.5,
    })
}

fn target_namespace(target: &ReclaimTarget) -> ObjectNamespaceBinding {
    match target {
        ReclaimTarget::Present { incarnation, .. } => incarnation.namespace,
        ReclaimTarget::AbsentProvisional(namespace) => *namespace,
    }
}

fn exact_retiring(row: &Row, retiring: RetiringRef) -> bool {
    row.phase == RETIRING
        && candidate(row) == retiring.candidate
        && row.direct_children == retiring.direct_children
}

fn exact_permit(row: &Row, expected: &ReclaimPermit, phase: i64) -> bool {
    row.phase == phase
        && candidate(row) == expected.retiring.candidate
        && row.direct_children == expected.retiring.direct_children
        && row.target.as_ref() == Some(&expected.target)
}

fn metadata(
    connection: &Connection,
    row: &Row,
    budget: &mut WorkBudget,
) -> Result<RegisteredObject, IndexError> {
    let table = match row.kind {
        ReclaimKind::Registered => "native_object_v1",
        ReclaimKind::Provisional => "native_object_install_v1",
    };
    let key = object_key(row.object);
    budget.charge(WorkUsage {
        reads: 1,
        ..WorkUsage::default()
    })?;
    let query = format!("SELECT length(payload),length(row_digest) FROM {table} WHERE object=?1");
    let sizes: Option<(i64, i64)> = connection
        .query_row(&query, params![&key], |record| {
            Ok((record.get(0)?, record.get(1)?))
        })
        .optional()
        .map_err(|error| sql(&error))?;
    let Some((size, checksum_len)) = sizes else {
        return Err(IndexError::Storage(
            "reclaim object registration missing".into(),
        ));
    };
    if !(1..=object_install::MAX_METADATA_BYTES_SQL).contains(&size) || checksum_len != 32 {
        return Err(IndexError::Storage(
            "reclaim registration shape invalid".into(),
        ));
    }
    budget.charge(WorkUsage {
        reads: 1,
        bytes: uint(size)?
            .checked_add(32)
            .ok_or(IndexError::ResourceLimited)?,
        ..WorkUsage::default()
    })?;
    let query = format!("SELECT payload,row_digest FROM {table} WHERE object=?1");
    let (payload, checksum): (Vec<u8>, Vec<u8>) = connection
        .query_row(&query, params![&key], |record| {
            Ok((record.get(0)?, record.get(1)?))
        })
        .map_err(|error| sql(&error))?;
    if payload.len() != usize::try_from(size).map_err(|_error| IndexError::ResourceLimited)? {
        return Err(IndexError::Storage("reclaim registration changed".into()));
    }
    let domain = match row.kind {
        ReclaimKind::Registered => b"qidx.native.object.v1".as_slice(),
        ReclaimKind::Provisional => b"qidx.native.install.v1".as_slice(),
    };
    check_digest(domain, &payload, &checksum)?;
    let registration: RegisteredObject = decode(&payload)?;
    if registration.reference != row.object {
        return Err(IndexError::Storage(
            "reclaim registration identity mismatch".into(),
        ));
    }
    let canonical = object_install::checked_object(&registration)?;
    if canonical != registration {
        return Err(IndexError::Storage(
            "reclaim registration is not canonical".into(),
        ));
    }
    Ok(registration)
}

pub(super) fn enter(
    catalog: &SqliteCatalog,
    retiring: RetiringRef,
    target: ReclaimTarget,
    budget: &mut WorkBudget,
) -> Result<ReclaimPermit, IndexError> {
    budget.checkpoint()?;
    let mut guard = catalog.lock_native_budget(budget)?;
    let tx = begin(&mut guard)?;
    let mut row =
        charged_row(&tx, retiring.candidate.object, budget)?.ok_or(IndexError::Conflict)?;
    if !exact_retiring(&row, retiring) || !queued(&tx, row.object)? {
        return Err(IndexError::Conflict);
    }
    if reference_exists(&tx, row.object, budget)? {
        return Err(IndexError::Conflict);
    }
    if target_namespace(&target) != bound_namespace(&tx)? {
        return Err(IndexError::Conflict);
    }
    match &target {
        ReclaimTarget::Present {
            incarnation,
            registration,
        } => {
            if incarnation.bytes != registration.bytes {
                return Err(IndexError::Conflict);
            }
            if registration.reference != row.object {
                return Err(IndexError::Conflict);
            }
            if *registration != metadata(&tx, &row, budget)? {
                return Err(IndexError::CorruptObject(
                    "reclaim target metadata changed".into(),
                ));
            }
        }
        ReclaimTarget::AbsentProvisional(_) => {
            if row.kind != ReclaimKind::Provisional {
                return Err(IndexError::Conflict);
            }
            let registered: Option<i64> = tx
                .query_row(
                    "SELECT 1 FROM native_object_v1 WHERE object=?1",
                    params![object_key(row.object)],
                    |record| record.get(0),
                )
                .optional()
                .map_err(|error| sql(&error))?;
            if registered.is_some() {
                return Err(IndexError::Conflict);
            }
            let _registration = metadata(&tx, &row, budget)?;
        }
    }
    row.target = Some(target);
    row.phase = RECLAIMING;
    budget.charge(WorkUsage {
        writes: 1,
        mutations: 1,
        ..WorkUsage::default()
    })?;
    update_row(&tx, &row)?;
    budget.checkpoint()?;
    tx.commit().map_err(|error| sql(&error))?;
    drop(guard);
    permit(&row)
}

pub(super) fn validate(
    catalog: &SqliteCatalog,
    expected: &ReclaimPermit,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    budget.checkpoint()?;
    let guard = catalog.lock_native_budget(budget)?;
    budget.charge(WorkUsage {
        reads: 2,
        ..WorkUsage::default()
    })?;
    let row = charged_row(&guard, expected.retiring.candidate.object, budget)?
        .ok_or(IndexError::Conflict)?;
    if !exact_permit(&row, expected, RECLAIMING)
        || target_namespace(&expected.target) != bound_namespace(&guard)?
    {
        return Err(IndexError::Conflict);
    }
    drop(guard);
    Ok(())
}

pub(super) fn mark_unlinked(
    catalog: &SqliteCatalog,
    expected: &ReclaimPermit,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    budget.checkpoint()?;
    let mut guard = catalog.lock_native_budget(budget)?;
    let tx = begin(&mut guard)?;
    let mut row = charged_row(&tx, expected.retiring.candidate.object, budget)?
        .ok_or(IndexError::Conflict)?;
    if exact_permit(&row, expected, DRAINING) {
        return Ok(());
    }
    if !exact_permit(&row, expected, RECLAIMING)
        || target_namespace(&expected.target) != bound_namespace(&tx)?
    {
        return Err(IndexError::Conflict);
    }
    row.phase = DRAINING;
    budget.charge(WorkUsage {
        writes: 1,
        mutations: 1,
        ..WorkUsage::default()
    })?;
    update_row(&tx, &row)?;
    budget.checkpoint()?;
    tx.commit().map_err(|error| sql(&error))?;
    drop(guard);
    Ok(())
}

pub(super) fn drain(
    catalog: &SqliteCatalog,
    expected: &ReclaimPermit,
    limit: u32,
    budget: &mut WorkBudget,
) -> Result<ReclaimDrainPage, IndexError> {
    if !(1..=MAX_RECLAIM_DRAIN_PAGE).contains(&limit) {
        return Err(IndexError::InvalidInput(
            "reclaim drain limit out of bounds".into(),
        ));
    }
    budget.checkpoint()?;
    let mut guard = catalog.lock_native_budget(budget)?;
    let tx = begin(&mut guard)?;
    let mut row = charged_row(&tx, expected.retiring.candidate.object, budget)?
        .ok_or(IndexError::Conflict)?;
    if !exact_permit(&row, expected, DRAINING)
        || target_namespace(&expected.target) != bound_namespace(&tx)?
    {
        return Err(IndexError::Conflict);
    }
    let table = match row.kind {
        ReclaimKind::Registered => "native_object_edge_v1",
        ReclaimKind::Provisional => "native_object_install_edge_v1",
    };
    let key = object_key(row.object);
    let registration = metadata(&tx, &row, budget)?;
    let mut expected_children = registration.children.clone();
    expected_children.sort_unstable();
    expected_children.dedup();
    if expected_children.len()
        != usize::try_from(row.direct_children).map_err(|_error| IndexError::ResourceLimited)?
    {
        return Err(IndexError::Storage(
            "frozen reclaim child count changed".into(),
        ));
    }
    let take = limit.checked_add(1).ok_or(IndexError::ResourceLimited)?;
    let query = format!(
        "SELECT CASE WHEN length(child)=32 THEN child END, CASE WHEN length(row_digest)=32 THEN row_digest END FROM {table} WHERE parent=?1 ORDER BY child LIMIT ?2"
    );
    budget.charge(WorkUsage {
        reads: u64::from(take),
        bytes: u64::from(take)
            .checked_mul(64)
            .ok_or(IndexError::ResourceLimited)?,
        ..WorkUsage::default()
    })?;
    let mut statement = tx.prepare(&query).map_err(|error| sql(&error))?;
    let records = statement
        .query_map(params![&key, i64::from(take)], |record| {
            Ok((
                record.get::<_, Option<Vec<u8>>>(0)?,
                record.get::<_, Option<Vec<u8>>>(1)?,
            ))
        })
        .map_err(|error| sql(&error))?;
    let mut edges = Vec::new();
    for record in records {
        let (child, checksum) = record.map_err(|error| sql(&error))?;
        let child =
            child.ok_or_else(|| IndexError::Storage("reclaim child key shape invalid".into()))?;
        let checksum = checksum
            .ok_or_else(|| IndexError::Storage("reclaim child digest shape invalid".into()))?;
        let domain = if row.kind == ReclaimKind::Registered {
            b"qidx.native.edge.v1".as_slice()
        } else {
            b"qidx.native.installedge.v1".as_slice()
        };
        if checksum != fields_digest(domain, &(&key, &child))? {
            return Err(IndexError::Storage(
                "reclaim child edge checksum mismatch".into(),
            ));
        }
        edges.push(child);
    }
    drop(statement);
    let consumed =
        usize::try_from(row.drained_children).map_err(|_error| IndexError::ResourceLimited)?;
    let expected_remaining = expected_children
        .len()
        .checked_sub(consumed)
        .ok_or_else(|| IndexError::Storage("reclaim drain cursor exceeds registration".into()))?;
    if edges.len() > expected_remaining {
        return Err(IndexError::Storage(
            "reclaim edge ledger gained an extra child".into(),
        ));
    }
    for (offset, child) in edges.iter().enumerate() {
        let index = consumed
            .checked_add(offset)
            .ok_or(IndexError::ResourceLimited)?;
        let expected = expected_children.get(index).ok_or_else(|| {
            IndexError::Storage("reclaim child page exceeds frozen registration suffix".into())
        })?;
        if *child != object_key(*expected) {
            return Err(IndexError::Storage(
                "reclaim child page differs from frozen registration suffix".into(),
            ));
        }
    }
    let max = usize::try_from(limit).map_err(|_error| IndexError::ResourceLimited)?;
    let has_more = edges.len() > max;
    if has_more != (expected_remaining > max) || (!has_more && edges.len() != expected_remaining) {
        return Err(IndexError::Storage(
            "reclaim child page omitted an edge".into(),
        ));
    }
    edges.truncate(max);
    let count = u64::try_from(edges.len()).map_err(|_error| IndexError::ResourceLimited)?;
    budget.charge(WorkUsage {
        reads: count.checked_mul(4).ok_or(IndexError::ResourceLimited)?,
        bytes: count.checked_mul(128).ok_or(IndexError::ResourceLimited)?,
        writes: count
            .checked_mul(2)
            .and_then(|v| v.checked_add(3))
            .ok_or(IndexError::ResourceLimited)?,
        mutations: count.checked_add(3).ok_or(IndexError::ResourceLimited)?,
    })?;
    let mut enqueued = 0_u32;
    for child in &edges {
        budget.checkpoint()?;
        let query = format!("DELETE FROM {table} WHERE parent=?1 AND child=?2");
        let deleted = tx
            .execute(&query, params![&key, child])
            .map_err(|error| sql(&error))?;
        exactly_one(deleted, "release reclaim child edge")?;
        let reference = from_key(child)?;
        if enqueue(&tx, reference)? {
            enqueued = enqueued.checked_add(1).ok_or(IndexError::ResourceLimited)?;
        }
    }
    row.drained_children = row
        .drained_children
        .checked_add(u32::try_from(edges.len()).map_err(|_error| IndexError::ResourceLimited)?)
        .ok_or(IndexError::ResourceLimited)?;
    if has_more {
        update_row(&tx, &row)?;
    } else {
        if row.drained_children != row.direct_children {
            return Err(IndexError::Storage(
                "reclaim drain omitted a child edge".into(),
            ));
        }
        if reference_exists(&tx, row.object, budget)? {
            return Err(IndexError::Storage(
                "reclaim parent gained custody while draining".into(),
            ));
        }
        let table = match row.kind {
            ReclaimKind::Registered => "native_object_v1",
            ReclaimKind::Provisional => "native_object_install_v1",
        };
        let query = format!("DELETE FROM {table} WHERE object=?1");
        exactly_one(
            tx.execute(&query, params![&key])
                .map_err(|error| sql(&error))?,
            "remove drained object metadata",
        )?;
        dequeue(&tx, row.object)?;
        // The durable global fence counter, rather than a per-object tombstone,
        // prevents a completed permit from matching a later installation.
        // Drop queue custody first to preserve its lifecycle foreign key.
        exactly_one(
            tx.execute(
                "DELETE FROM native_gc_object_v1 WHERE object=?1 AND generation=?2 AND phase=?3",
                params![&key, int(row.generation)?, DRAINING],
            )
            .map_err(|error| sql(&error))?,
            "remove completed object lifecycle",
        )?;
    }
    budget.checkpoint()?;
    tx.commit().map_err(|error| sql(&error))?;
    drop(guard);
    Ok(ReclaimDrainPage {
        released_children: u32::try_from(edges.len())
            .map_err(|_error| IndexError::ResourceLimited)?,
        enqueued_children: enqueued,
        has_more,
    })
}

#[cfg(test)]
mod target_registration_tests {
    use super::{
        IndexError, ObjectNamespaceBinding, PhysicalIncarnation, ReclaimTarget, RegisteredObject,
        StoredTarget, decode_target, target_bytes,
    };
    use quanta_index_core::index::{Digest, MAX_RECLAIM_CHILDREN, ObjectRef};

    macro_rules! check {
        ($condition:expr $(,)?) => {{
            if !$condition {
                return Err(format!("assertion failed: {}", stringify!($condition)).into());
            }
        }};
    }

    macro_rules! check_eq {
        ($left:expr, $right:expr $(,)?) => {{
            let left = &$left;
            let right = &$right;
            if left != right {
                return Err(format!("assertion failed: {left:?} != {right:?}").into());
            }
        }};
    }

    fn target(children: usize) -> ReclaimTarget {
        let namespace = ObjectNamespaceBinding {
            root_device: 1,
            root_inode: 2,
            objects_device: 1,
            objects_inode: 3,
            lease_device: 1,
            lease_inode: 4,
        };
        ReclaimTarget::Present {
            incarnation: PhysicalIncarnation {
                namespace,
                shard_device: 1,
                shard_inode: 5,
                device: 1,
                inode: 6,
                changed_nanos: 1,
                bytes: 128,
            },
            registration: RegisteredObject::unscoped(
                ObjectRef(Digest([7; 32])),
                128,
                vec![ObjectRef(Digest([8; 32])); children],
            ),
        }
    }

    fn wire_unchecked(target: &ReclaimTarget) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        let ReclaimTarget::Present {
            incarnation,
            registration,
        } = target
        else {
            return Err("expected present target".into());
        };
        let namespace = incarnation.namespace;
        Ok(serde_json::to_vec(&StoredTarget {
            present: true,
            root_device: namespace.root_device,
            root_inode: namespace.root_inode,
            objects_device: namespace.objects_device,
            objects_inode: namespace.objects_inode,
            lease_device: namespace.lease_device,
            lease_inode: namespace.lease_inode,
            shard_device: incarnation.shard_device,
            shard_inode: incarnation.shard_inode,
            file_device: incarnation.device,
            file_inode: incarnation.inode,
            file_changed_nanos: incarnation.changed_nanos,
            file_bytes: incarnation.bytes,
            registration: Some(registration.clone()),
        })?)
    }

    #[test]
    fn persisted_target_rejects_registration_exceeding_direct_child_cap()
    -> Result<(), Box<dyn std::error::Error>> {
        let boundary = target(MAX_RECLAIM_CHILDREN);
        let boundary_wire = wire_unchecked(&boundary)?;
        check_eq!(decode_target(&boundary_wire)?, boundary);
        let oversized = target(MAX_RECLAIM_CHILDREN.saturating_add(1));
        let oversized_wire = wire_unchecked(&oversized)?;
        check!(matches!(
            decode_target(&oversized_wire),
            Err(IndexError::Storage(_))
        ));
        check!(matches!(
            target_bytes(Some(&oversized)),
            Err(IndexError::ResourceLimited)
        ));
        Ok(())
    }

    #[test]
    fn persisted_target_rejects_noncanonical_registration_shape()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut zero_bytes = target(1);
        if let ReclaimTarget::Present { registration, .. } = &mut zero_bytes {
            *registration = RegisteredObject::unscoped(
                registration.reference,
                0,
                registration.children.clone(),
            );
        }
        check!(matches!(
            decode_target(&wire_unchecked(&zero_bytes)?),
            Err(IndexError::Storage(_))
        ));
        let mut self_child = target(1);
        if let ReclaimTarget::Present { registration, .. } = &mut self_child {
            *registration = RegisteredObject::unscoped(
                registration.reference,
                registration.bytes,
                vec![registration.reference],
            );
        }
        check!(matches!(
            decode_target(&wire_unchecked(&self_child)?),
            Err(IndexError::Storage(_))
        ));
        Ok(())
    }
    #[test]
    fn persisted_target_must_match_frozen_direct_child_count()
    -> Result<(), Box<dyn std::error::Error>> {
        let object = ObjectRef(Digest([7; 32]));
        let row = super::Row {
            object,
            generation: 1,
            kind: super::ReclaimKind::Registered,
            phase: super::RECLAIMING,
            direct_children: 2,
            drained_children: 0,
            target: Some(target(1)),
        };
        let encoded = target_bytes(row.target.as_ref())?;
        let checksum = super::row_digest(&row, encoded.as_ref())?;
        let connection = rusqlite::Connection::open_in_memory()?;
        connection.execute_batch(
            "CREATE TABLE native_gc_object_v1(\
                object BLOB PRIMARY KEY, generation INTEGER, kind INTEGER, phase INTEGER,\
                direct_children INTEGER, drained_children INTEGER, target BLOB, row_digest BLOB\
            )",
        )?;
        let changed = connection.execute(
            "INSERT INTO native_gc_object_v1 VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            rusqlite::params![
                super::object_key(object),
                1_i64,
                super::REGISTERED,
                super::RECLAIMING,
                2_i64,
                0_i64,
                encoded,
                checksum,
            ],
        )?;
        check_eq!(changed, 1);
        check!(matches!(
            super::read_row(&connection, object),
            Err(IndexError::Storage(_))
        ));
        Ok(())
    }

    #[test]
    fn persisted_duplicate_children_resume_with_distinct_edge_count()
    -> Result<(), Box<dyn std::error::Error>> {
        let object = ObjectRef(Digest([7; 32]));
        // The physical registration preserves both occurrences, while the
        // custody edge table and frozen direct-child count contain one key.
        let duplicated = target(2);
        let row = super::Row {
            object,
            generation: 1,
            kind: super::ReclaimKind::Registered,
            phase: super::RECLAIMING,
            direct_children: 1,
            drained_children: 0,
            target: Some(duplicated.clone()),
        };
        let encoded = target_bytes(row.target.as_ref())?;
        let checksum = super::row_digest(&row, encoded.as_ref())?;
        let connection = rusqlite::Connection::open_in_memory()?;
        connection.execute_batch(
            "CREATE TABLE native_gc_object_v1(\
                object BLOB PRIMARY KEY, generation INTEGER, kind INTEGER, phase INTEGER,\
                direct_children INTEGER, drained_children INTEGER, target BLOB, row_digest BLOB\
            )",
        )?;
        let changed = connection.execute(
            "INSERT INTO native_gc_object_v1 VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            rusqlite::params![
                super::object_key(object),
                1_i64,
                super::REGISTERED,
                super::RECLAIMING,
                1_i64,
                0_i64,
                encoded,
                checksum,
            ],
        )?;
        check_eq!(changed, 1);
        let resumed = super::read_row(&connection, object)?.ok_or("reclaim row missing")?;
        check_eq!(resumed.direct_children, 1);
        check_eq!(resumed.target, Some(duplicated));
        Ok(())
    }
}
