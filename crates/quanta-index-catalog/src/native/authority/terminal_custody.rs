//! Release operation-local custody only after the original receipt is terminal.
//! Object registrations, edges, view/work roots and replay claims remain.

use quanta_index_core::index::ports::{
    CatalogCustodyMaintenancePort, TerminalCustodyCandidate, TerminalCustodyPage,
};
use quanta_index_core::index::{
    CollectionId, IndexError, ObjectRef, OperationKey, OperationStatus, ProducerId, SlotId,
    StreamId, TenantId, WorkBudget, WorkUsage, WorkspaceKey, WorktreeId,
};
use rusqlite::{Connection, OptionalExtension as _, params};

use super::{
    SqliteCatalog, begin, check_digest, decode, exactly_one, fields_digest, int,
    lifetime::max_workspace_key_bytes, object_install::MAX_METADATA_BYTES_SQL, object_key,
    operation_key, read_operation, sql, uint, workspace_key,
};

const MAX_PAGE_ROWS: u32 = 256;

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
    const fn table(self) -> &'static str {
        match self {
            Self::OperationObject => "native_operation_object_v1",
            Self::AttemptObject => "native_attempt_object_v1",
            Self::ObjectIntent => "native_object_intent_v1",
        }
    }

    const fn order(self) -> &'static str {
        match self {
            Self::OperationObject => "object",
            Self::AttemptObject | Self::ObjectIntent => "fence,object",
        }
    }
}

struct CustodyRow {
    kind: CustodyKind,
    fence: Option<i64>,
    object: Vec<u8>,
}

fn charged_bytes(lengths: &[i64]) -> Result<u64, IndexError> {
    lengths.iter().try_fold(0_u64, |total, &length| {
        total
            .checked_add(uint(length)?)
            .ok_or(IndexError::ResourceLimited)
    })
}

fn next_in_table(
    connection: &Connection,
    operation: &[u8],
    kind: CustodyKind,
    protected: &[Option<ObjectRef>; 5],
    budget: &mut WorkBudget,
) -> Result<Option<CustodyRow>, IndexError> {
    let blobs = protected.map(|slot| slot.map(|object| object.0.0));
    let [one, two, three, four, five] = blobs
        .each_ref()
        .map(|slot| slot.as_ref().map(<[u8; 32]>::as_slice));
    let filtered = matches!(kind, CustodyKind::OperationObject);
    let exclusion = if filtered {
        " AND (?2 IS NULL OR object<>?2) AND (?3 IS NULL OR object<>?3)
          AND (?4 IS NULL OR object<>?4) AND (?5 IS NULL OR object<>?5)
          AND (?6 IS NULL OR object<>?6)"
    } else {
        ""
    };
    let bound: &[&dyn rusqlite::ToSql] = if filtered {
        params![operation, one, two, three, four, five]
    } else {
        params![operation]
    };
    // Read sizes before allocating an intent payload or decoding its children.
    budget.charge(WorkUsage {
        reads: 1,
        ..WorkUsage::default()
    })?;
    let lengths_query = match kind {
        CustodyKind::ObjectIntent => format!(
            "SELECT CASE WHEN typeof(object)='blob' AND length(object)=32 THEN 32 END,
             CASE WHEN typeof(payload)='blob' AND length(payload) BETWEEN 1 AND {MAX_METADATA_BYTES_SQL} THEN length(payload) END,
             CASE WHEN typeof(row_digest)='blob' AND length(row_digest)=32 THEN 32 END
             FROM {} WHERE operation=?1{exclusion} ORDER BY {} LIMIT 1",
            kind.table(),
            kind.order()
        ),
        CustodyKind::OperationObject | CustodyKind::AttemptObject => format!(
            "SELECT CASE WHEN typeof(object)='blob' AND length(object)=32 THEN 32 END,
             CASE WHEN typeof(row_digest)='blob' AND length(row_digest)=32 THEN 32 END
             FROM {} WHERE operation=?1{exclusion} ORDER BY {} LIMIT 1",
            kind.table(),
            kind.order()
        ),
    };
    let lengths: Option<Vec<Option<i64>>> = match kind {
        CustodyKind::ObjectIntent => connection
            .query_row(&lengths_query, bound, |row| {
                Ok(vec![row.get(0)?, row.get(1)?, row.get(2)?])
            })
            .optional()
            .map_err(|error| sql(&error))?,
        CustodyKind::OperationObject | CustodyKind::AttemptObject => connection
            .query_row(&lengths_query, bound, |row| {
                Ok(vec![row.get(0)?, row.get(1)?])
            })
            .optional()
            .map_err(|error| sql(&error))?,
    };
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
    let query = match kind {
        CustodyKind::OperationObject => format!(
            "SELECT object,row_digest FROM {} WHERE operation=?1{exclusion} ORDER BY {} LIMIT 1",
            kind.table(),
            kind.order()
        ),
        CustodyKind::AttemptObject => format!(
            "SELECT fence,object,row_digest FROM {} WHERE operation=?1 ORDER BY {} LIMIT 1",
            kind.table(),
            kind.order()
        ),
        CustodyKind::ObjectIntent => format!(
            "SELECT fence,object,payload,row_digest FROM {} WHERE operation=?1 ORDER BY {} LIMIT 1",
            kind.table(),
            kind.order()
        ),
    };
    let (fence, object, payload, row_digest): (Option<i64>, Vec<u8>, Option<Vec<u8>>, Vec<u8>) =
        match kind {
            CustodyKind::OperationObject => connection
                .query_row(&query, bound, |row| {
                    Ok((None, row.get(0)?, None, row.get(1)?))
                })
                .map_err(|error| sql(&error))?,
            CustodyKind::AttemptObject => connection
                .query_row(&query, params![operation], |row| {
                    Ok((Some(row.get(0)?), row.get(1)?, None, row.get(2)?))
                })
                .map_err(|error| sql(&error))?,
            CustodyKind::ObjectIntent => connection
                .query_row(&query, params![operation], |row| {
                    Ok((
                        Some(row.get(0)?),
                        row.get(1)?,
                        Some(row.get(2)?),
                        row.get(3)?,
                    ))
                })
                .map_err(|error| sql(&error))?,
        };
    let actual_lengths = payload.as_ref().map_or_else(
        || vec![object.len(), row_digest.len()],
        |payload| vec![object.len(), payload.len(), row_digest.len()],
    );
    let expected_lengths = lengths
        .iter()
        .map(|&length| usize::try_from(length))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_error| IndexError::Storage("custody row length overflow".into()))?;
    let max_metadata_bytes =
        usize::try_from(MAX_METADATA_BYTES_SQL).map_err(|_error| IndexError::ResourceLimited)?;
    if actual_lengths != expected_lengths
        || object.len() != 32
        || row_digest.len() != 32
        || payload
            .as_ref()
            .is_some_and(|payload| payload.is_empty() || payload.len() > max_metadata_bytes)
    {
        return Err(IndexError::Storage("custody row shape mismatch".into()));
    }
    match kind {
        CustodyKind::OperationObject => {
            if row_digest != fields_digest(b"qidx.native.opobject.v1", &(operation, &object))? {
                return Err(IndexError::Storage(
                    "operation custody digest mismatch".into(),
                ));
            }
        }
        CustodyKind::AttemptObject => {
            let fence = fence.ok_or_else(|| IndexError::Storage("missing attempt fence".into()))?;
            if uint(fence)? == 0
                || row_digest
                    != fields_digest(
                        b"qidx.native.attemptobject.v1",
                        &(operation, fence, &object),
                    )?
            {
                return Err(IndexError::Storage(
                    "attempt custody digest mismatch".into(),
                ));
            }
        }
        CustodyKind::ObjectIntent => {
            let fence = fence.ok_or_else(|| IndexError::Storage("missing intent fence".into()))?;
            if uint(fence)? == 0 {
                return Err(IndexError::Storage("invalid intent fence".into()));
            }
            let payload =
                payload.ok_or_else(|| IndexError::Storage("missing intent payload".into()))?;
            check_digest(b"qidx.native.intent.v1", &payload, &row_digest)?;
            let registration: quanta_index_core::index::RegisteredObject = decode(&payload)?;
            if object_key(registration.reference) != object {
                return Err(IndexError::Storage("intent object key mismatch".into()));
            }
        }
    }
    Ok(Some(CustodyRow {
        kind,
        fence,
        object,
    }))
}

fn next_row(
    connection: &Connection,
    operation: &[u8],
    protected: &[Option<ObjectRef>; 5],
    budget: &mut WorkBudget,
) -> Result<Option<CustodyRow>, IndexError> {
    for kind in [
        CustodyKind::OperationObject,
        CustodyKind::AttemptObject,
        CustodyKind::ObjectIntent,
    ] {
        if let Some(row) = next_in_table(connection, operation, kind, protected, budget)? {
            return Ok(Some(row));
        }
    }
    Ok(None)
}

fn delete_row(
    connection: &Connection,
    operation: &[u8],
    row: &CustodyRow,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    budget.charge(WorkUsage {
        writes: 1,
        mutations: 1,
        ..WorkUsage::default()
    })?;
    let changed = match row.kind {
        CustodyKind::OperationObject => connection.execute(
            "DELETE FROM native_operation_object_v1 WHERE operation=?1 AND object=?2",
            params![operation, &row.object],
        ),
        CustodyKind::AttemptObject => connection.execute(
            "DELETE FROM native_attempt_object_v1 WHERE operation=?1 AND fence=?2 AND object=?3",
            params![operation, row.fence, &row.object],
        ),
        CustodyKind::ObjectIntent => connection.execute(
            "DELETE FROM native_object_intent_v1 WHERE operation=?1 AND fence=?2 AND object=?3",
            params![operation, row.fence, &row.object],
        ),
    }
    .map_err(|error| sql(&error))?;
    exactly_one(changed, "release terminal custody")?;
    if super::reclaim::is_bound(connection)? {
        budget.charge(WorkUsage {
            reads: 10,
            writes: 1,
            bytes: 640,
            mutations: 1,
        })?;
        super::reclaim::release_key(connection, &row.object)?;
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
        let tx = begin(&mut guard)?;
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
        let operation_lengths: Option<(i64, i64, i64)> = tx
            .query_row(
                "SELECT CASE WHEN typeof(workspace)='blob' AND length(workspace) BETWEEN 1 AND ?2 THEN length(workspace) END,
                 CASE WHEN typeof(payload)='blob' AND length(payload)>0 THEN length(payload) END,
                 CASE WHEN typeof(row_digest)='blob' AND length(row_digest)=32 THEN 32 END
                 FROM native_operation_v1 WHERE operation=?1",
                params![&operation, max_workspace_bytes],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .map_err(|error| sql(&error))?;
        let (workspace_bytes, payload_bytes, digest_bytes) =
            operation_lengths.ok_or(IndexError::SnapshotUnavailable)?;
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
        let mut released = 0;
        while released < limit {
            let Some(row) = next_row(&tx, &operation, &protected, budget)? else {
                break;
            };
            delete_row(&tx, &operation, &row, budget)?;
            released = released.checked_add(1).ok_or(IndexError::ResourceLimited)?;
        }
        // Validate the page+1 row before committing a purported has-more.
        let has_more = next_row(&tx, &operation, &protected, budget)?.is_some();
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
    fn oversized_intent_payload_is_rejected_before_allocation_or_deletion()
    -> Result<(), Box<dyn Error>> {
        let connection = Connection::open_in_memory()?;
        connection.execute_batch(
            "CREATE TABLE native_object_intent_v1 (
                operation BLOB, fence INTEGER, object BLOB, payload BLOB, row_digest BLOB
            );",
        )?;
        let operation = vec![7_u8; 32];
        let _changed = connection.execute(
            "INSERT INTO native_object_intent_v1 VALUES (?1,1,zeroblob(32),zeroblob(?2),zeroblob(32))",
            params![&operation, MAX_METADATA_BYTES_SQL + 1],
        )?;
        let mut ample = budget(2_000_000)?;
        check!(matches!(
            next_in_table(
                &connection,
                &operation,
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
                &operation,
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
                        &operation,
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
                &operation,
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
                &operation,
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
                &operation,
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
