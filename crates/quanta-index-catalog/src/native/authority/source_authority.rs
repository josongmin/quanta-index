//! Producer-wide source epoch authority. A source stream may roll over only
//! after one explicitly admitted resync publishes the producer epoch fence.

use quanta_index_core::index::ports::SourceAuthorityPort;
use quanta_index_core::index::{
    IndexError, OperationKey, ProducerId, SourceEvent, SourceOperationMode, ViewRef, WorkBudget,
    WorkUsage, WorkspaceKey,
};
use rusqlite::{Connection, OptionalExtension as _, params};

use super::{
    SqliteCatalog, current_view, exactly_one, fields_digest, int, sql, uint, workspace_key,
};

const AUTHORITY_DOMAIN: &[u8] = b"qidx.native.sourceauthority.v1";

fn charge_row(
    budget: &mut WorkBudget,
    workspace_bytes: usize,
    reads: u64,
    writes: u64,
) -> Result<(), IndexError> {
    // Covers the encoded workspace/producer keys, bounded row digest and
    // canonical JSON tuple used to verify the digest on this one indexed row.
    let bytes = u64::try_from(workspace_bytes)
        .map_err(|_error| IndexError::ResourceLimited)?
        .checked_add(16)
        .and_then(|size| size.checked_mul(8))
        .and_then(|size| size.checked_add(256))
        .ok_or(IndexError::ResourceLimited)?;
    budget.charge(WorkUsage {
        reads,
        writes,
        bytes,
        ..WorkUsage::default()
    })
}

fn authority_digest(
    workspace: &[u8],
    workspace_epoch: i64,
    producer: &[u8],
    source_epoch: i64,
) -> Result<Vec<u8>, IndexError> {
    fields_digest(
        AUTHORITY_DOMAIN,
        &(workspace, workspace_epoch, producer, source_epoch),
    )
}

fn read_epoch(
    connection: &Connection,
    workspace: WorkspaceKey,
    workspace_epoch: u64,
    producer: ProducerId,
    budget: &mut WorkBudget,
) -> Result<Option<u64>, IndexError> {
    let workspace = workspace_key(workspace)?;
    charge_row(budget, workspace.len(), 1, 0)?;
    let workspace_epoch = int(workspace_epoch)?;
    let producer = producer.0.to_vec();
    let row: Option<(i64, Vec<u8>)> = connection
        .query_row(
            "SELECT source_epoch,row_digest FROM native_source_authority_v1
             WHERE workspace=?1 AND workspace_epoch=?2 AND producer=?3",
            params![&workspace, workspace_epoch, &producer],
            |row| {
                let value = row.get_ref(1)?;
                let rusqlite::types::ValueRef::Blob(digest) = value else {
                    return Err(rusqlite::Error::InvalidColumnType(
                        1,
                        "row_digest".into(),
                        value.data_type(),
                    ));
                };
                if digest.len() != 32 {
                    return Err(rusqlite::Error::InvalidColumnType(
                        1,
                        "row_digest".into(),
                        rusqlite::types::Type::Blob,
                    ));
                }
                Ok((row.get(0)?, digest.to_vec()))
            },
        )
        .optional()
        .map_err(|error| sql(&error))?;
    let Some((source_epoch, digest)) = row else {
        return Ok(None);
    };
    if digest != authority_digest(&workspace, workspace_epoch, &producer, source_epoch)? {
        return Err(IndexError::Storage(
            "native producer source epoch digest mismatch".into(),
        ));
    }
    Ok(Some(uint(source_epoch)?))
}

fn insert_epoch(
    connection: &Connection,
    workspace: WorkspaceKey,
    workspace_epoch: u64,
    producer: ProducerId,
    source_epoch: u64,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    let workspace = workspace_key(workspace)?;
    charge_row(budget, workspace.len(), 0, 1)?;
    let workspace_epoch = int(workspace_epoch)?;
    let producer = producer.0.to_vec();
    let source_epoch = int(source_epoch)?;
    let inserted = connection
        .execute(
            "INSERT INTO native_source_authority_v1
             (workspace,workspace_epoch,producer,source_epoch,row_digest)
             VALUES (?1,?2,?3,?4,?5)",
            params![
                &workspace,
                workspace_epoch,
                &producer,
                source_epoch,
                authority_digest(&workspace, workspace_epoch, &producer, source_epoch)?
            ],
        )
        .map_err(|error| sql(&error))?;
    exactly_one(inserted, "create producer source epoch")
}

fn resync_event(
    operation: OperationKey,
    prior_epoch: u64,
    events: &[SourceEvent],
) -> Result<&SourceEvent, IndexError> {
    let [event] = events else {
        return Err(IndexError::InvalidInput(
            "source resync requires one bound source event".into(),
        ));
    };
    if event.producer != operation.producer
        || event.stream != operation.stream
        || event.stream_epoch != operation.stream_epoch
        || event.stream_epoch <= prior_epoch
        || event.predecessor != 0
        || event.sequence != 1
    {
        return Err(IndexError::InvalidInput(
            "source resync event does not bind a greater producer epoch baseline".into(),
        ));
    }
    Ok(event)
}

pub(super) fn admit(
    connection: &Connection,
    operation: OperationKey,
    mode: SourceOperationMode,
    events: &[SourceEvent],
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    match mode {
        SourceOperationMode::Unchanged => Ok(()),
        SourceOperationMode::Advance => {
            if events.is_empty() {
                return Err(IndexError::InvalidInput(
                    "source advance requires an event".into(),
                ));
            }
            for event in events {
                match read_epoch(
                    connection,
                    operation.workspace,
                    operation.epoch,
                    event.producer,
                    budget,
                )? {
                    Some(epoch) if epoch == event.stream_epoch => {}
                    Some(_) => return Err(IndexError::Conflict),
                    None => insert_epoch(
                        connection,
                        operation.workspace,
                        operation.epoch,
                        event.producer,
                        event.stream_epoch,
                        budget,
                    )?,
                }
            }
            Ok(())
        }
        SourceOperationMode::Resync { prior_epoch } => {
            let event = resync_event(operation, prior_epoch, events)?;
            if read_epoch(
                connection,
                operation.workspace,
                operation.epoch,
                event.producer,
                budget,
            )? != Some(prior_epoch)
            {
                return Err(IndexError::Conflict);
            }
            Ok(())
        }
    }
}

pub(super) fn commit(
    connection: &Connection,
    operation: OperationKey,
    mode: SourceOperationMode,
    events: &[SourceEvent],
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    match mode {
        SourceOperationMode::Unchanged => {
            if events.is_empty() {
                Ok(())
            } else {
                Err(IndexError::InvalidInput(
                    "unchanged source mode cannot publish source events".into(),
                ))
            }
        }
        SourceOperationMode::Advance => {
            if events.is_empty() {
                return Err(IndexError::InvalidInput(
                    "source advance requires an event".into(),
                ));
            }
            for event in events {
                if read_epoch(
                    connection,
                    operation.workspace,
                    operation.epoch,
                    event.producer,
                    budget,
                )? != Some(event.stream_epoch)
                {
                    return Err(IndexError::Conflict);
                }
            }
            Ok(())
        }
        SourceOperationMode::Resync { prior_epoch } => {
            let event = resync_event(operation, prior_epoch, events)?;
            if read_epoch(
                connection,
                operation.workspace,
                operation.epoch,
                event.producer,
                budget,
            )? != Some(prior_epoch)
            {
                return Err(IndexError::Conflict);
            }
            let workspace = workspace_key(operation.workspace)?;
            charge_row(budget, workspace.len(), 0, 1)?;
            let workspace_epoch = int(operation.epoch)?;
            let producer = event.producer.0.to_vec();
            let prior_epoch = int(prior_epoch)?;
            let next_epoch = int(event.stream_epoch)?;
            let changed = connection
                .execute(
                    "UPDATE native_source_authority_v1 SET source_epoch=?4,row_digest=?5
                     WHERE workspace=?1 AND workspace_epoch=?2 AND producer=?3 AND source_epoch=?6",
                    params![
                        &workspace,
                        workspace_epoch,
                        &producer,
                        next_epoch,
                        authority_digest(&workspace, workspace_epoch, &producer, next_epoch)?,
                        prior_epoch
                    ],
                )
                .map_err(|error| sql(&error))?;
            if changed != 1 {
                return Err(IndexError::Conflict);
            }
            Ok(())
        }
    }
}

impl SourceAuthorityPort for SqliteCatalog {
    fn source_epoch(
        &self,
        view: ViewRef,
        producer: ProducerId,
        budget: &mut WorkBudget,
    ) -> Result<Option<u64>, IndexError> {
        budget.checkpoint()?;
        let guard = self.lock_native_budget(budget)?;
        let current =
            current_view(&guard, view.workspace)?.ok_or(IndexError::SnapshotUnavailable)?;
        if current.identity != view {
            return Err(IndexError::Conflict);
        }
        let result = read_epoch(&guard, view.workspace, view.epoch, producer, budget);
        drop(guard);
        result
    }
}
