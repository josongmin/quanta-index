//! Release operation-local custody only after the original receipt is terminal.
//! Object registrations, edges, view/work roots and replay claims remain.

use quanta_index_core::index::ports::{
    CatalogCustodyMaintenancePort, TerminalCustodyCandidate, TerminalCustodyPage,
};
use quanta_index_core::index::{
    CollectionId, IndexError, ObjectRef, OperationKey, OperationStatus, ProducerId, SlotId,
    StreamId, TenantId, WorkBudget, WorkUsage, WorkspaceKey, WorktreeId,
};
use rusqlite::{Connection, OptionalExtension as _, TransactionBehavior, params, types::ValueRef};

use super::registration_codec::decode as decode_registration;
use super::{
    SqliteCatalog, begin, cached_execute, check_digest, decode, exactly_one, fields_digest, int,
    lifetime::max_workspace_key_bytes, object_install::MAX_METADATA_BYTES_SQL, object_key,
    operation_key, read_operation, sql, uint, workspace_key,
};

const MAX_PAGE_ROWS: u32 = 256;
const MAX_FIXED_PREFETCH_ROWS: usize = 32;

// Derive the bound from the canonical serializer rather than a byte guess.
// All-255 IDs and maximum unsigned counters have the widest JSON spelling.
pub(super) fn widest_operation_key() -> OperationKey {
    OperationKey {
        workspace: WorkspaceKey {
            tenant: TenantId([u8::MAX; 16]),
            collection: CollectionId([u8::MAX; 16]),
            worktree: WorktreeId([u8::MAX; 16]),
            slot: SlotId([u8::MAX; 16]),
        },
        epoch: u64::MAX,
        producer: ProducerId([u8::MAX; 16]),
        stream: StreamId([u8::MAX; 16]),
        stream_epoch: u64::MAX,
        sequence: u64::MAX,
    }
}

fn max_operation_key_bytes() -> Result<usize, IndexError> {
    Ok(operation_key(widest_operation_key())?.len())
}
pub(super) const READY_AFTER_QUERY: &str =
    "SELECT ready_sequence,operation,row_digest FROM native_terminal_custody_v1
    WHERE workspace=?1 AND epoch=?2 AND ready_sequence>COALESCE(?3,0)
    ORDER BY ready_sequence LIMIT 1";
pub(super) const READY_PREFIX_QUERY: &str =
    "SELECT ready_sequence,operation,row_digest FROM native_terminal_custody_v1
    WHERE workspace=?1 AND epoch=?2 AND ready_sequence>COALESCE(?3,0)
    AND ready_sequence<=?4 ORDER BY ready_sequence LIMIT 1";

#[derive(Clone, Copy)]
enum CustodyKind {
    OperationObject,
    AttemptObject,
    ObjectIntent,
}

impl CustodyKind {
    const fn next(self) -> Option<Self> {
        match self {
            Self::OperationObject => Some(Self::AttemptObject),
            Self::AttemptObject => Some(Self::ObjectIntent),
            Self::ObjectIntent => None,
        }
    }
}

#[derive(Clone, Copy)]
struct CustodyRow {
    kind: CustodyKind,
    fence: Option<i64>,
    object: [u8; 32],
}

struct FixedCustodyChunk {
    rows: [Option<CustodyRow>; MAX_FIXED_PREFETCH_ROWS],
    len: usize,
}

#[derive(Clone, Copy)]
struct CustodyOperation<'a> {
    id: i64,
    key: &'a [u8],
}

fn charged_bytes(lengths: &[i64]) -> Result<u64, IndexError> {
    lengths.iter().try_fold(0_u64, |total, &length| {
        total
            .checked_add(uint(length)?)
            .ok_or(IndexError::ResourceLimited)
    })
}

const NEXT_OPERATION_OBJECT: &str =
    "SELECT CASE WHEN typeof(object)='blob' AND length(object)=32 THEN object END,
            CASE WHEN typeof(row_digest)='blob' AND length(row_digest)=32 THEN row_digest END
     FROM native_operation_object_v1
     WHERE operation_id=?1
       AND (?2 IS NULL OR object<>?2) AND (?3 IS NULL OR object<>?3)
       AND (?4 IS NULL OR object<>?4) AND (?5 IS NULL OR object<>?5)
       AND (?6 IS NULL OR object<>?6)
     ORDER BY object LIMIT ?7";
const NEXT_ATTEMPT_OBJECT: &str = "SELECT fence,
            CASE WHEN typeof(object)='blob' AND length(object)=32 THEN object END,
            CASE WHEN typeof(row_digest)='blob' AND length(row_digest)=32 THEN row_digest END
     FROM native_attempt_object_v1
     WHERE operation_id=?1
     ORDER BY fence,object LIMIT ?2";

/// Fixed-width custody needs one bounded borrowed read. Reject malformed
/// blobs before copying, while preserving the original ordering and digests.
fn next_fixed_chunk(
    connection: &Connection,
    operation: CustodyOperation<'_>,
    kind: CustodyKind,
    protected: &[Option<ObjectRef>; 5],
    limit: usize,
    budget: &mut WorkBudget,
) -> Result<FixedCustodyChunk, IndexError> {
    if !(1..=MAX_FIXED_PREFETCH_ROWS).contains(&limit) {
        return Err(IndexError::BackendViolation(
            "invalid fixed custody prefetch limit".into(),
        ));
    }
    let sql_limit = i64::try_from(limit).map_err(|_error| IndexError::ResourceLimited)?;
    let blobs = protected.map(|slot| slot.map(|object| object.0.0));
    let [one, two, three, four, five] = blobs
        .each_ref()
        .map(|slot| slot.as_ref().map(<[u8; 32]>::as_slice));
    let operation_bound: &[&dyn rusqlite::ToSql] =
        params![operation.id, one, two, three, four, five, sql_limit];
    let attempt_bound: &[&dyn rusqlite::ToSql] = params![operation.id, sql_limit];
    let (query, bound, object_slot, digest_slot): (&str, &[&dyn rusqlite::ToSql], usize, usize) =
        match kind {
            CustodyKind::OperationObject => (NEXT_OPERATION_OBJECT, operation_bound, 0, 1),
            CustodyKind::AttemptObject => (NEXT_ATTEMPT_OBJECT, attempt_bound, 1, 2),
            CustodyKind::ObjectIntent => {
                return Err(IndexError::BackendViolation(
                    "intent row entered fixed-width custody reader".into(),
                ));
            }
        };
    budget.charge(WorkUsage {
        reads: 1,
        ..WorkUsage::default()
    })?;
    let mut statement = connection
        .prepare_cached(query)
        .map_err(|error| sql(&error))?;
    let mut rows = statement.query(bound).map_err(|error| sql(&error))?;
    let mut chunk = FixedCustodyChunk {
        rows: [None; MAX_FIXED_PREFETCH_ROWS],
        len: 0,
    };
    for (index, slot) in chunk.rows.iter_mut().take(limit).enumerate() {
        // Charge each logical row read, including the empty read that advances
        // the kind. The single SQL cursor avoids one seek per custody row.
        if index != 0 {
            budget.charge(WorkUsage {
                reads: 1,
                ..WorkUsage::default()
            })?;
        }
        let Some(row) = rows.next().map_err(|error| sql(&error))? else {
            break;
        };
        let fence = if matches!(kind, CustodyKind::AttemptObject) {
            Some(row.get::<_, i64>(0).map_err(|error| sql(&error))?)
        } else {
            None
        };
        let ValueRef::Blob(object) = row.get_ref(object_slot).map_err(|error| sql(&error))? else {
            return Err(IndexError::Storage("custody row shape mismatch".into()));
        };
        let ValueRef::Blob(row_digest) = row.get_ref(digest_slot).map_err(|error| sql(&error))?
        else {
            return Err(IndexError::Storage("custody row shape mismatch".into()));
        };
        if object.len() != 32 || row_digest.len() != 32 {
            return Err(IndexError::Storage("custody row shape mismatch".into()));
        }
        budget.charge(WorkUsage {
            bytes: 64,
            ..WorkUsage::default()
        })?;
        match kind {
            CustodyKind::OperationObject => {
                if row_digest
                    != fields_digest(b"qidx.native.opobject.v1", &(operation.key, object))?
                        .as_slice()
                {
                    return Err(IndexError::Storage(
                        "operation custody digest mismatch".into(),
                    ));
                }
            }
            CustodyKind::AttemptObject => {
                let fence =
                    fence.ok_or_else(|| IndexError::Storage("missing attempt fence".into()))?;
                if uint(fence)? == 0
                    || row_digest
                        != fields_digest(
                            b"qidx.native.attemptobject.v1",
                            &(operation.key, fence, object),
                        )?
                        .as_slice()
                {
                    return Err(IndexError::Storage(
                        "attempt custody digest mismatch".into(),
                    ));
                }
            }
            CustodyKind::ObjectIntent => {
                return Err(IndexError::BackendViolation(
                    "intent row entered fixed-width custody reader".into(),
                ));
            }
        }
        let object: [u8; 32] = object
            .try_into()
            .map_err(|_error| IndexError::Storage("custody row shape mismatch".into()))?;
        *slot = Some(CustodyRow {
            kind,
            fence,
            object,
        });
        chunk.len = chunk
            .len
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
    }
    Ok(chunk)
}

fn next_fixed_row(
    connection: &Connection,
    operation: CustodyOperation<'_>,
    kind: CustodyKind,
    protected: &[Option<ObjectRef>; 5],
    budget: &mut WorkBudget,
) -> Result<Option<CustodyRow>, IndexError> {
    let chunk = next_fixed_chunk(connection, operation, kind, protected, 1, budget)?;
    Ok(chunk.rows.first().copied().flatten())
}

fn next_intent_row(
    connection: &Connection,
    operation_id: i64,
    budget: &mut WorkBudget,
) -> Result<Option<CustodyRow>, IndexError> {
    // Intent payloads are variable sized. Preflight their exact lengths before
    // copying or decoding, unlike the fixed-width custody rows above.
    budget.charge(WorkUsage {
        reads: 1,
        ..WorkUsage::default()
    })?;
    let lengths_query = format!(
        "SELECT CASE WHEN typeof(object)='blob' AND length(object)=32 THEN 32 END,
             CASE WHEN typeof(payload)='blob' AND length(payload) BETWEEN 1 AND {MAX_METADATA_BYTES_SQL} THEN length(payload) END,
             CASE WHEN typeof(row_digest)='blob' AND length(row_digest)=32 THEN 32 END
         FROM native_object_intent_v1
         WHERE operation_id=?1
         ORDER BY fence,object LIMIT 1"
    );
    let lengths: Option<Vec<Option<i64>>> = connection
        .query_row(&lengths_query, params![operation_id], |row| {
            Ok(vec![row.get(0)?, row.get(1)?, row.get(2)?])
        })
        .optional()
        .map_err(|error| sql(&error))?;
    let Some(lengths) = lengths else {
        return Ok(None);
    };
    let lengths: Vec<i64> = lengths
        .into_iter()
        .collect::<Option<_>>()
        .ok_or_else(|| IndexError::Storage("custody row shape mismatch".into()))?;
    budget.charge(WorkUsage {
        reads: 1,
        bytes: charged_bytes(&lengths)?,
        ..WorkUsage::default()
    })?;
    let (fence, object, payload, row_digest): (i64, Vec<u8>, Vec<u8>, Vec<u8>) = connection
        .query_row(
            "SELECT fence,object,payload,row_digest FROM native_object_intent_v1
             WHERE operation_id=?1
             ORDER BY fence,object LIMIT 1",
            params![operation_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .map_err(|error| sql(&error))?;
    let actual_lengths = [object.len(), payload.len(), row_digest.len()];
    let expected_lengths = lengths
        .iter()
        .map(|&length| usize::try_from(length))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_error| IndexError::Storage("custody row length overflow".into()))?;
    let max_metadata_bytes =
        usize::try_from(MAX_METADATA_BYTES_SQL).map_err(|_error| IndexError::ResourceLimited)?;
    if actual_lengths.as_slice() != expected_lengths.as_slice()
        || object.len() != 32
        || row_digest.len() != 32
        || payload.is_empty()
        || payload.len() > max_metadata_bytes
    {
        return Err(IndexError::Storage("custody row shape mismatch".into()));
    }
    if uint(fence)? == 0 {
        return Err(IndexError::Storage("invalid intent fence".into()));
    }
    check_digest(b"qidx.native.intent.v1", &payload, &row_digest)?;
    let registration: quanta_index_core::index::RegisteredObject = decode_registration(&payload)?;
    if object_key(registration.reference) != object {
        return Err(IndexError::Storage("intent object key mismatch".into()));
    }
    let object: [u8; 32] = object
        .try_into()
        .map_err(|_error| IndexError::Storage("custody row shape mismatch".into()))?;
    Ok(Some(CustodyRow {
        kind: CustodyKind::ObjectIntent,
        fence: Some(fence),
        object,
    }))
}

fn next_in_table(
    connection: &Connection,
    operation: CustodyOperation<'_>,
    kind: CustodyKind,
    protected: &[Option<ObjectRef>; 5],
    budget: &mut WorkBudget,
) -> Result<Option<CustodyRow>, IndexError> {
    match kind {
        CustodyKind::ObjectIntent => next_intent_row(connection, operation.id, budget),
        CustodyKind::OperationObject | CustodyKind::AttemptObject => {
            next_fixed_row(connection, operation, kind, protected, budget)
        }
    }
}

fn next_row(
    connection: &Connection,
    operation: CustodyOperation<'_>,
    protected: &[Option<ObjectRef>; 5],
    kind: &mut Option<CustodyKind>,
    budget: &mut WorkBudget,
) -> Result<Option<CustodyRow>, IndexError> {
    while let Some(current) = *kind {
        if let Some(row) = next_in_table(connection, operation, current, protected, budget)? {
            return Ok(Some(row));
        }
        *kind = current.next();
    }
    Ok(None)
}

fn delete_row(
    transaction: &rusqlite::Transaction<'_>,
    operation_id: i64,
    row: &CustodyRow,
    reclaim_bound: &mut Option<bool>,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    budget.charge(WorkUsage {
        writes: 1,
        mutations: 1,
        ..WorkUsage::default()
    })?;
    let changed = match row.kind {
        CustodyKind::OperationObject => cached_execute(
            transaction,
            "DELETE FROM native_operation_object_v1 WHERE operation_id=?1 AND object=?2",
            params![operation_id, row.object.as_slice()],
        ),
        CustodyKind::AttemptObject => cached_execute(
            transaction,
            "DELETE FROM native_attempt_object_v1 WHERE operation_id=?1 AND fence=?2 AND object=?3",
            params![operation_id, row.fence, row.object.as_slice()],
        ),
        CustodyKind::ObjectIntent => cached_execute(
            transaction,
            "DELETE FROM native_object_intent_v1 WHERE operation_id=?1 AND fence=?2 AND object=?3",
            params![operation_id, row.fence, row.object.as_slice()],
        ),
    }
    .map_err(|error| sql(&error))?;
    exactly_one(changed, "release terminal custody")?;
    // The namespace row cannot change within this writer transaction.
    // Authenticate it lazily once, then reuse the original enqueue path.
    let bound = if let Some(bound) = *reclaim_bound {
        bound
    } else {
        let bound = super::reclaim::is_bound(transaction)?;
        *reclaim_bound = Some(bound);
        bound
    };
    if bound {
        budget.charge(WorkUsage {
            reads: 10,
            writes: 1,
            bytes: 640,
            mutations: 1,
        })?;
        let object = super::reclaim::from_key(row.object.as_slice())?;
        let _queued = super::reclaim::enqueue(transaction, object)?;
    }
    Ok(())
}

impl CatalogCustodyMaintenancePort for SqliteCatalog {
    fn next_terminal_custody(
        &self,
        workspace: WorkspaceKey,
        epoch: u64,
        after: Option<u64>,
        through: Option<u64>,
        budget: &mut WorkBudget,
    ) -> Result<Option<TerminalCustodyCandidate>, IndexError> {
        // Canonical keys are bounded independently of historical receipt
        // payloads. Inspect borrowed SQL values before copying the ready row.
        let maximum_key_bytes = max_operation_key_bytes()?;
        budget.charge(WorkUsage {
            reads: 1,
            bytes: u64::try_from(maximum_key_bytes)
                .map_err(|_error| IndexError::ResourceLimited)?
                .checked_add(32 + 8)
                .ok_or(IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;
        let workspace_blob = workspace_key(workspace)?;
        let epoch_integer = int(epoch)?;
        let after_integer = after.map(int).transpose()?;
        let through_integer = through.map(int).transpose()?;
        let mut guard = self.lock_native_budget(budget)?;
        // Selection is advisory. Release reauthenticates the operation in
        // its own writer transaction, so this snapshot needs no writer slot.
        let tx = guard
            .transaction_with_behavior(TransactionBehavior::Deferred)
            .map_err(|error| sql(&error))?;
        let read_ready = |row: &rusqlite::Row<'_>| {
            let ready_sequence = row.get::<_, i64>(0)?;
            let operation = row.get_ref(1)?.as_blob()?;
            let checksum = row.get_ref(2)?.as_blob()?;
            if operation.is_empty() || operation.len() > maximum_key_bytes || checksum.len() != 32 {
                return Err(rusqlite::Error::InvalidQuery);
            }
            Ok((ready_sequence, operation.to_vec(), checksum.to_vec()))
        };
        // Keep both range bounds indexable. An optional-bound OR predicate
        // can scan every higher key when a frozen head prefix is exhausted.
        let selected = through_integer.as_ref().map_or_else(
            || {
                tx.query_row(
                    READY_AFTER_QUERY,
                    params![&workspace_blob, epoch_integer, after_integer],
                    read_ready,
                )
            },
            |upper| {
                tx.query_row(
                    READY_PREFIX_QUERY,
                    params![&workspace_blob, epoch_integer, after_integer, upper],
                    read_ready,
                )
            },
        );
        let row: Option<(i64, Vec<u8>, Vec<u8>)> =
            selected.optional().map_err(|error| sql(&error))?;
        let candidate = if let Some((ready_sequence, operation, checksum)) = row {
            let sequence = uint(ready_sequence)?;
            if sequence == 0
                || after.is_some_and(|prior| sequence <= prior)
                || through.is_some_and(|upper| sequence > upper)
            {
                return Err(IndexError::Storage(
                    "terminal ready sequence mismatch".into(),
                ));
            }
            if checksum
                != fields_digest(
                    b"qidx.native.terminal-custody.v1",
                    &(ready_sequence, &operation, &workspace_blob, epoch_integer),
                )?
            {
                return Err(IndexError::Storage(
                    "terminal custody ready digest mismatch".into(),
                ));
            }
            let key: OperationKey = decode(&operation)?;
            if operation_key(key)? != operation || key.workspace != workspace || key.epoch != epoch
            {
                return Err(IndexError::Storage(
                    "terminal custody ready key mismatch".into(),
                ));
            }
            budget.charge(WorkUsage {
                reads: 1,
                ..WorkUsage::default()
            })?;
            let parent: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM native_operation_v1 WHERE operation=?1 AND workspace=?2 AND epoch=?3)",
                params![&operation, &workspace_blob, epoch_integer], |row| row.get(0),
            ).map_err(|error| sql(&error))?;
            if !parent {
                return Err(IndexError::Storage(
                    "terminal custody ready parent missing".into(),
                ));
            }
            Some(TerminalCustodyCandidate {
                sequence,
                operation: key,
            })
        } else {
            None
        };
        budget.checkpoint()?;
        tx.commit().map_err(|error| sql(&error))?;
        drop(guard);
        Ok(candidate)
    }

    fn release_terminal_custody(
        &self,
        key: OperationKey,
        limit: u32,
        budget: &mut WorkBudget,
    ) -> Result<TerminalCustodyPage, IndexError> {
        if !(1..=MAX_PAGE_ROWS).contains(&limit) {
            return Err(IndexError::InvalidInput(
                "terminal custody page limit must be in 1..=256".into(),
            ));
        }
        budget.checkpoint()?;
        let operation = operation_key(key)?;
        let max_workspace_bytes = max_workspace_key_bytes()?;
        let mut guard = self.lock_native_budget(budget)?;
        let tx = begin(&mut guard)?;
        let protected = super::boot_manifest::protected_roots(&tx, key, budget)?;
        budget.charge(WorkUsage {
            reads: 1,
            ..WorkUsage::default()
        })?;
        let operation_header: Option<(i64, i64, i64, i64)> = tx
            .query_row(
                "SELECT operation_id,
                 CASE WHEN typeof(workspace)='blob' AND length(workspace) BETWEEN 1 AND ?2 THEN length(workspace) END,
                 CASE WHEN typeof(payload)='blob' AND length(payload)>0 THEN length(payload) END,
                 CASE WHEN typeof(row_digest)='blob' AND length(row_digest)=32 THEN 32 END
                 FROM native_operation_v1 WHERE operation=?1",
                params![&operation, max_workspace_bytes],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()
            .map_err(|error| sql(&error))?;
        let (operation_id, workspace_bytes, payload_bytes, digest_bytes) =
            operation_header.ok_or(IndexError::SnapshotUnavailable)?;
        if operation_id <= 0 {
            return Err(IndexError::Storage(
                "terminal custody operation id is invalid".into(),
            ));
        }
        budget.charge(WorkUsage {
            reads: 1,
            bytes: charged_bytes(&<[i64; 3]>::from((
                workspace_bytes,
                payload_bytes,
                digest_bytes,
            )))?,
            ..WorkUsage::default()
        })?;
        let row = read_operation(&tx, key)?.ok_or(IndexError::SnapshotUnavailable)?;
        match row.status {
            OperationStatus::Published(_) | OperationStatus::Refused(_) => {}
            OperationStatus::Pending(_) => return Err(IndexError::Conflict),
            OperationStatus::Unknown => {
                return Err(IndexError::Storage(
                    "stored operation has unknown status".into(),
                ));
            }
        }
        // This writer transaction fixes the authenticated key-to-id mapping.
        // Row digests keep the canonical key; SQL uses the numeric primary key.
        let custody = CustodyOperation {
            id: operation_id,
            key: &operation,
        };
        let mut released = 0;
        let mut kind = Some(CustodyKind::OperationObject);
        let mut reclaim_bound = None;
        while released < limit {
            if let Some(current @ (CustodyKind::OperationObject | CustodyKind::AttemptObject)) =
                kind
            {
                let remaining = limit
                    .checked_sub(released)
                    .ok_or(IndexError::ResourceLimited)?;
                let requested = usize::try_from(remaining)
                    .map_err(|_error| IndexError::ResourceLimited)?
                    .min(MAX_FIXED_PREFETCH_ROWS);
                let chunk = next_fixed_chunk(&tx, custody, current, &protected, requested, budget)?;
                for row in chunk.rows.into_iter().take(chunk.len) {
                    let row = row.ok_or_else(|| {
                        IndexError::BackendViolation("fixed custody chunk gap".into())
                    })?;
                    delete_row(&tx, operation_id, &row, &mut reclaim_bound, budget)?;
                    released = released.checked_add(1).ok_or(IndexError::ResourceLimited)?;
                }
                if chunk.len < requested {
                    kind = current.next();
                }
                continue;
            }
            let Some(row) = next_row(&tx, custody, &protected, &mut kind, budget)? else {
                break;
            };
            delete_row(&tx, operation_id, &row, &mut reclaim_bound, budget)?;
            released = released.checked_add(1).ok_or(IndexError::ResourceLimited)?;
        }
        // Validate the page+1 row before committing a purported has-more.
        let has_more = next_row(&tx, custody, &protected, &mut kind, budget)?.is_some();
        if !has_more {
            budget.charge(WorkUsage {
                writes: 1,
                mutations: 1,
                ..WorkUsage::default()
            })?;
            let _removed = tx
                .execute(
                    "DELETE FROM native_terminal_custody_v1 WHERE operation=?1",
                    params![&operation],
                )
                .map_err(|error| sql(&error))?;
        }
        budget.checkpoint()?;
        tx.commit().map_err(|error| sql(&error))?;
        drop(guard);
        Ok(TerminalCustodyPage { released, has_more })
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::time::{Duration, Instant};

    use quanta_index_core::index::WorkLimits;

    use super::*;

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

    fn budget(bytes: u64) -> Result<WorkBudget, IndexError> {
        Ok(WorkBudget::new(
            WorkLimits {
                reads: 10,
                writes: 0,
                bytes,
                mutations: 0,
            },
            Instant::now()
                .checked_add(Duration::from_secs(30))
                .ok_or(IndexError::ResourceLimited)?,
        ))
    }

    #[test]
    fn fixed_custody_rows_charge_before_copy_and_reject_malformed_blobs()
    -> Result<(), Box<dyn Error>> {
        let connection = Connection::open_in_memory()?;
        connection.execute_batch(
            "CREATE TABLE native_operation_v1 (operation_id INTEGER PRIMARY KEY, operation BLOB UNIQUE);
             CREATE TABLE native_operation_object_v1 (operation_id INTEGER, object BLOB, row_digest BLOB);
             CREATE TABLE native_attempt_object_v1 (operation_id INTEGER, fence INTEGER, object BLOB, row_digest BLOB);",
        )?;
        let operation = vec![7_u8; 32];
        let object = vec![8_u8; 32];
        let _inserted = connection.execute(
            "INSERT INTO native_operation_v1 VALUES (1,?1)",
            params![&operation],
        )?;
        let _inserted = connection.execute(
            "INSERT INTO native_operation_object_v1 VALUES (1,?1,?2)",
            params![
                &object,
                fields_digest(b"qidx.native.opobject.v1", &(operation.as_slice(), &object))?
            ],
        )?;
        let _inserted = connection.execute(
            "INSERT INTO native_attempt_object_v1 VALUES (1,1,?1,?2)",
            params![
                &object,
                fields_digest(
                    b"qidx.native.attemptobject.v1",
                    &(operation.as_slice(), 1_i64, &object)
                )?
            ],
        )?;
        let custody = CustodyOperation {
            id: 1,
            key: &operation,
        };
        for kind in [CustodyKind::OperationObject, CustodyKind::AttemptObject] {
            let mut funded = budget(64)?;
            let row = next_in_table(&connection, custody, kind, &[None; 5], &mut funded)?
                .ok_or("fixed custody row missing")?;
            check_eq!(row.object.as_slice(), object.as_slice());
            check_eq!(funded.usage().bytes, 64);
            let wrong_key = CustodyOperation {
                id: 1,
                key: &[9_u8; 32],
            };
            check!(matches!(
                next_in_table(&connection, wrong_key, kind, &[None; 5], &mut budget(64)?),
                Err(IndexError::Storage(message)) if message.contains("custody digest mismatch")
            ));
        }
        let _changed = connection.execute(
            "UPDATE native_operation_object_v1 SET row_digest=zeroblob(1048577)",
            [],
        )?;
        let mut funded = budget(64)?;
        check!(matches!(
            next_in_table(
                &connection,
                custody,
                CustodyKind::OperationObject,
                &[None; 5],
                &mut funded
            ),
            Err(IndexError::Storage(_))
        ));
        check_eq!(funded.usage().bytes, 0);
        let _changed = connection.execute(
            "UPDATE native_attempt_object_v1 SET object=zeroblob(33)",
            [],
        )?;
        let mut funded = budget(64)?;
        check!(matches!(
            next_in_table(
                &connection,
                custody,
                CustodyKind::AttemptObject,
                &[None; 5],
                &mut funded
            ),
            Err(IndexError::Storage(_))
        ));
        check_eq!(funded.usage().bytes, 0);
        Ok(())
    }

    #[test]
    fn fixed_custody_prefetch_is_ordered_bounded_and_closes_cursor() -> Result<(), Box<dyn Error>> {
        let connection = Connection::open_in_memory()?;
        connection.execute_batch(
            "CREATE TABLE native_operation_object_v1 (
                operation_id INTEGER, object BLOB, row_digest BLOB
            );",
        )?;
        let operation = [7_u8; 32];
        for value in 1..=33_u8 {
            let object = [value; 32];
            let digest = fields_digest(
                b"qidx.native.opobject.v1",
                &(operation.as_slice(), object.as_slice()),
            )?;
            let _inserted = connection.execute(
                "INSERT INTO native_operation_object_v1 VALUES (1,?1,?2)",
                params![object.as_slice(), digest],
            )?;
        }
        let custody = CustodyOperation {
            id: 1,
            key: &operation,
        };
        let mut funded = WorkBudget::new(
            WorkLimits {
                reads: 40,
                writes: 0,
                bytes: 3_000,
                mutations: 0,
            },
            Instant::now()
                .checked_add(Duration::from_secs(30))
                .ok_or(IndexError::ResourceLimited)?,
        );
        let first = next_fixed_chunk(
            &connection,
            custody,
            CustodyKind::OperationObject,
            &[None; 5],
            32,
            &mut funded,
        )?;
        check_eq!(first.len, 32);
        check_eq!(funded.usage().reads, 32);
        for (ordinal, fixed) in first.rows.into_iter().take(first.len).enumerate() {
            let fixed = fixed.ok_or("fixed custody chunk gap")?;
            check_eq!(fixed.object, [u8::try_from(ordinal)?.saturating_add(1); 32]);
            let _deleted = connection.execute(
                "DELETE FROM native_operation_object_v1 WHERE operation_id=1 AND object=?1",
                params![fixed.object.as_slice()],
            )?;
        }
        let last = next_fixed_chunk(
            &connection,
            custody,
            CustodyKind::OperationObject,
            &[None; 5],
            32,
            &mut funded,
        )?;
        check_eq!(last.len, 1);
        check_eq!(
            last.rows
                .first()
                .copied()
                .flatten()
                .ok_or("last custody row missing")?
                .object,
            [33; 32]
        );
        check_eq!(funded.usage().reads, 34);
        check_eq!(funded.usage().bytes, 2_112);
        check!(matches!(
            next_fixed_chunk(
                &connection,
                custody,
                CustodyKind::OperationObject,
                &[None; 5],
                0,
                &mut funded
            ),
            Err(IndexError::BackendViolation(_))
        ));
        Ok(())
    }

    #[test]
    fn oversized_intent_payload_is_rejected_before_allocation_or_deletion()
    -> Result<(), Box<dyn Error>> {
        let connection = Connection::open_in_memory()?;
        connection.execute_batch(
            "CREATE TABLE native_operation_v1 (
                operation_id INTEGER PRIMARY KEY NOT NULL,
                operation BLOB UNIQUE NOT NULL
            );
            CREATE TABLE native_object_intent_v1 (
                operation_id INTEGER NOT NULL, fence INTEGER, object BLOB,
                payload BLOB, row_digest BLOB,
                FOREIGN KEY(operation_id) REFERENCES native_operation_v1(operation_id)
            );",
        )?;
        let operation = vec![7_u8; 32];
        let _parent = connection.execute(
            "INSERT INTO native_operation_v1(operation_id,operation) VALUES (1,?1)",
            params![&operation],
        )?;
        let _changed = connection.execute(
            "INSERT INTO native_object_intent_v1 VALUES (1,1,zeroblob(32),zeroblob(?1),zeroblob(32))",
            params![MAX_METADATA_BYTES_SQL + 1],
        )?;
        let custody = CustodyOperation {
            id: 1,
            key: &operation,
        };
        let mut ample = budget(2_000_000)?;
        check!(matches!(
            next_in_table(
                &connection,
                custody,
                CustodyKind::ObjectIntent,
                &[None; 5],
                &mut ample
            ),
            Err(IndexError::Storage(_))
        ));
        check_eq!(ample.usage().bytes, 0);
        let count: i64 =
            connection.query_row("SELECT count(*) FROM native_object_intent_v1", [], |row| {
                row.get(0)
            })?;
        check_eq!(count, 1);

        let _changed = connection.execute("UPDATE native_object_intent_v1 SET payload='x'", [])?;
        check!(matches!(
            next_in_table(
                &connection,
                custody,
                CustodyKind::ObjectIntent,
                &[None; 5],
                &mut budget(1024)?
            ),
            Err(IndexError::Storage(_))
        ));

        for column in ["object", "row_digest"] {
            for corrupt_value in [
                "zeroblob(31)",
                "zeroblob(33)",
                "zeroblob(1048577)",
                "'text'",
            ] {
                let _changed = connection.execute(
                    "UPDATE native_object_intent_v1 SET object=zeroblob(32),payload=zeroblob(1),row_digest=zeroblob(32)",
                    [],
                )?;
                let _changed = connection.execute(
                    &format!("UPDATE native_object_intent_v1 SET {column}={corrupt_value}"),
                    [],
                )?;
                let mut ample = budget(2_000_000)?;
                check!(matches!(
                    next_in_table(
                        &connection,
                        custody,
                        CustodyKind::ObjectIntent,
                        &[None; 5],
                        &mut ample
                    ),
                    Err(IndexError::Storage(_))
                ));
                check_eq!(ample.usage().bytes, 0);
            }
        }
        let _changed = connection.execute("UPDATE native_object_intent_v1 SET object=zeroblob(32),row_digest=zeroblob(32),payload=zeroblob(0)", [])?;
        let mut ample = budget(2_000_000)?;
        check!(matches!(
            next_in_table(
                &connection,
                custody,
                CustodyKind::ObjectIntent,
                &[None; 5],
                &mut ample
            ),
            Err(IndexError::Storage(_))
        ));
        check_eq!(ample.usage().bytes, 0);

        let _changed =
            connection.execute("UPDATE native_object_intent_v1 SET payload=zeroblob(1)", [])?;
        let mut exhausted = budget(64)?;
        check!(matches!(
            next_in_table(
                &connection,
                custody,
                CustodyKind::ObjectIntent,
                &[None; 5],
                &mut exhausted
            ),
            Err(IndexError::ResourceLimited)
        ));
        check_eq!(exhausted.usage().bytes, 0);

        let request =
            quanta_index_core::RequestBudgetV1::until(Instant::now() + Duration::from_secs(30));
        request.cancel_handle().cancel();
        let mut cancelled = WorkBudget::with_request(
            WorkLimits {
                reads: 10,
                writes: 0,
                bytes: 1024,
                mutations: 0,
            },
            request,
        );
        check!(matches!(
            next_in_table(
                &connection,
                custody,
                CustodyKind::ObjectIntent,
                &[None; 5],
                &mut cancelled
            ),
            Err(IndexError::Cancelled)
        ));
        let count: i64 =
            connection.query_row("SELECT count(*) FROM native_object_intent_v1", [], |row| {
                row.get(0)
            })?;
        check_eq!(count, 1);
        Ok(())
    }
}
