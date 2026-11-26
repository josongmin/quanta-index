//! Durable exact publication result. This is operation metadata, not a view pin.

use quanta_index_contract::native_protocol::{NATIVE_DSL_PRODUCER_V1, native_dsl_sequence_v1};
use quanta_index_core::index::{
    CatalogView, IndexError, MemoryLease, MemoryLifetime, OperationKey, OperationStatus,
    WorkBudget, WorkUsage,
};
use rusqlite::{Connection, OptionalExtension as _, TransactionBehavior, params};

use super::{OperationRow, SqliteCatalog, operation_key, read_operation_with_budget, sql, uint};

pub(super) fn validate_operation_result(row: &OperationRow) -> Result<(), IndexError> {
    match (&row.status, row.published_view.as_ref()) {
        (OperationStatus::Published(identity), Some(view)) => {
            if view.identity != *identity
                || view.identity.workspace != row.immutable.key.workspace
                || view.identity.epoch != row.immutable.key.epoch
                || view.identity.digest
                    != view.identity_digest().map_err(|error| {
                        IndexError::Storage(format!(
                            "native publication result digest encoding failed: {error}"
                        ))
                    })?
            {
                return Err(IndexError::Storage(
                    "native publication result identity mismatch".into(),
                ));
            }
            Ok(())
        }
        (OperationStatus::Pending(_) | OperationStatus::Refused(_), None) => Ok(()),
        _ => Err(IndexError::Storage(
            "native operation publication result status mismatch".into(),
        )),
    }
}

pub(super) fn read_published_metadata(
    catalog: &SqliteCatalog,
    key: OperationKey,
    budget: &mut WorkBudget,
) -> Result<CatalogView, IndexError> {
    with_operation_metadata(catalog, key, budget, |row| {
        let row = row.ok_or(IndexError::SnapshotUnavailable)?;
        let view = match (row.status, row.published_view) {
            (OperationStatus::Published(_), Some(view)) => Ok(view),
            (OperationStatus::Pending(_) | OperationStatus::Refused(_), None) => {
                Err(IndexError::Conflict)
            }
            (OperationStatus::Unknown, _) => Err(IndexError::SnapshotUnavailable),
            (OperationStatus::Published(_), None)
            | (OperationStatus::Pending(_) | OperationStatus::Refused(_), Some(_)) => Err(
                IndexError::Storage("native operation publication result status mismatch".into()),
            ),
        }?;
        if key.producer == NATIVE_DSL_PRODUCER_V1 {
            let replay = row.projection_replay.ok_or(IndexError::DigestConflict)?;
            if replay.source != view.source
                || replay.projection.0 != key.stream.0
                || key.stream_epoch != 1
                || native_dsl_sequence_v1(replay.context) == 0
            {
                return Err(IndexError::DigestConflict);
            }
        }
        Ok(view)
    })
}

/// Project the operation row while its temporary allocation is reserved.
pub(super) fn with_operation_metadata<T>(
    catalog: &SqliteCatalog,
    key: OperationKey,
    budget: &mut WorkBudget,
    project: impl FnOnce(Option<OperationRow>) -> Result<T, IndexError>,
) -> Result<T, IndexError> {
    let _key_memory = reserve_operation_probe(budget)?;
    let mut guard = catalog.lock_native_budget(budget)?;
    let transaction = guard
        .transaction_with_behavior(TransactionBehavior::Deferred)
        .map_err(|error| sql(&error))?;
    let result = project_operation_row(&transaction, key, budget, project)?;
    transaction.commit().map_err(|error| sql(&error))?;
    drop(guard);
    Ok(result)
}

/// Read a bounded operation row inside a caller-owned Catalog transaction.
pub(super) fn project_operation_metadata<T>(
    connection: &Connection,
    key: OperationKey,
    budget: &mut WorkBudget,
    project: impl FnOnce(Option<OperationRow>) -> Result<T, IndexError>,
) -> Result<T, IndexError> {
    let _key_memory = reserve_operation_probe(budget)?;
    project_operation_row(connection, key, budget, project)
}

fn reserve_operation_probe(budget: &mut WorkBudget) -> Result<Option<MemoryLease>, IndexError> {
    // The key is fixed-shape; reserve its encoding and the SQLite statement
    // before allocating either. The payload itself is admitted by its stored
    // length below, with no corpus- or boot-manifest-sized hard cap.
    budget.charge(WorkUsage {
        reads: 1,
        bytes: 1024,
        ..WorkUsage::default()
    })?;
    budget.reserve_memory(1024, MemoryLifetime::Temporary)
}

fn project_operation_row<T>(
    connection: &Connection,
    key: OperationKey,
    budget: &mut WorkBudget,
    project: impl FnOnce(Option<OperationRow>) -> Result<T, IndexError>,
) -> Result<T, IndexError> {
    let encoded_key = operation_key(key)?;
    let lengths: Option<(Option<i64>, Option<i64>, Option<i64>)> = connection
        .query_row(
            "SELECT CASE WHEN typeof(workspace)='blob' THEN length(workspace) END,
                    CASE WHEN typeof(payload)='blob' THEN length(payload) END,
                    CASE WHEN typeof(row_digest)='blob' THEN length(row_digest) END
             FROM native_operation_v1 WHERE operation=?1",
            params![encoded_key],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(|error| sql(&error))?;
    let Some(lengths) = lengths else {
        budget.checkpoint()?;
        return project(None);
    };
    let (Some(workspace_length), Some(payload_length), Some(digest_length)) = lengths else {
        return Err(IndexError::Storage(
            "native operation metadata column type mismatch".into(),
        ));
    };
    if workspace_length <= 0 || payload_length <= 0 || digest_length != 32 {
        return Err(IndexError::Storage(
            "native operation metadata column length mismatch".into(),
        ));
    }
    let workspace_length = uint(workspace_length)?;
    let payload_length = uint(payload_length)?;
    let payload_work = workspace_length
        .checked_add(payload_length)
        .and_then(|value| value.checked_add(32))
        // CatalogView has only fixed-width fields. This bound pays for both
        // canonical identity-digest serialization passes in read_operation.
        .and_then(|value| value.checked_add(8192))
        .ok_or(IndexError::ResourceLimited)?;
    budget.charge(WorkUsage {
        reads: 1,
        bytes: payload_work,
        ..WorkUsage::default()
    })?;
    // Logical temporary-memory envelope: SQLite materializes the payload and
    // workspace BLOBs, then the canonical decoder owns fixed record fields
    // plus a Vec<SourceEvent>. Each fixed event's canonical JSON uses more
    // bytes than its in-memory fields; four payload lengths cover Vec growth
    // as well as simultaneous input and decoded ownership. This is admission
    // accounting, not a claim about the system allocator's exact footprint.
    let scratch = payload_length
        .checked_mul(4)
        .and_then(|value| value.checked_add(workspace_length))
        .and_then(|value| value.checked_add(4096))
        .ok_or(IndexError::ResourceLimited)?;
    let _row_memory = budget.reserve_memory(scratch, MemoryLifetime::Temporary)?;
    let result = project(read_operation_with_budget(connection, key, Some(budget))?)?;
    budget.checkpoint()?;
    Ok(result)
}
