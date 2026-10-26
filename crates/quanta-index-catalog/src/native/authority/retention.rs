//! Explicit Exact-admission floor and bounded retirement of catalog view rows.

use quanta_index_core::index::ports::{CatalogExactHistoryPort, ViewRetirementPage};
use quanta_index_core::index::{
    CatalogView, Digest, IndexError, ViewRef, WorkBudget, WorkUsage, WorkspaceKey,
};
use rusqlite::{Connection, OptionalExtension as _, params};

use super::{
    SqliteCatalog, begin, current_view, distinct_roots, exactly_one, fields_digest, int,
    read_view_row, sql, uint, workspace_key,
};

const FLOOR_DIGEST_DOMAIN: &[u8] = b"qidx.native.exactfloor.v1";
const MAX_RETIRE_VIEWS: u32 = 256;

fn row_bytes(parts: &[i64]) -> Result<u64, IndexError> {
    parts.iter().try_fold(0_u64, |total, &part| {
        total
            .checked_add(uint(part)?)
            .ok_or(IndexError::ResourceLimited)
    })
}

fn floor_row(
    connection: &Connection,
    workspace: WorkspaceKey,
    epoch: u64,
) -> Result<Option<u64>, IndexError> {
    let workspace_blob = workspace_key(workspace)?;
    let epoch_integer = int(epoch)?;
    let row: Option<(i64, Vec<u8>)> = connection
        .query_row(
            "SELECT retain_from_sequence,row_digest FROM native_exact_history_floor_v1 WHERE workspace=?1 AND epoch=?2",
            params![&workspace_blob, epoch_integer],
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
    let Some((stored, checksum)) = row else {
        return Ok(None);
    };
    if checksum
        != fields_digest(
            FLOOR_DIGEST_DOMAIN,
            &(&workspace_blob, epoch_integer, stored),
        )?
    {
        return Err(IndexError::Storage(
            "exact history floor digest mismatch".into(),
        ));
    }
    let floor = uint(stored)?;
    let head: Option<i64> = connection
        .query_row(
            "SELECT sequence FROM native_view_v1 WHERE workspace=?1 AND epoch=?2 ORDER BY sequence DESC LIMIT 1",
            params![&workspace_blob, epoch_integer],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| sql(&error))?;
    let head = head.ok_or_else(|| IndexError::Storage("floor epoch has no view".into()))?;
    if floor > uint(head)? {
        return Err(IndexError::Storage(
            "exact history floor exceeds epoch head".into(),
        ));
    }
    Ok(Some(floor))
}

/// Used by Exact acquisition in the same transaction that installs its pin.
pub(super) fn exact_floor_for_epoch(
    connection: &Connection,
    workspace: WorkspaceKey,
    epoch: u64,
) -> Result<u64, IndexError> {
    Ok(floor_row(connection, workspace, epoch)?.unwrap_or(0))
}

pub(super) fn charged_current_view(
    connection: &Connection,
    workspace: WorkspaceKey,
    budget: &mut WorkBudget,
) -> Result<Option<quanta_index_core::index::CatalogView>, IndexError> {
    let workspace_blob = workspace_key(workspace)?;
    budget.charge(WorkUsage {
        reads: 1,
        ..WorkUsage::default()
    })?;
    let metadata: Option<(i64, i64, i64, i64)> = connection
        .query_row(
            "SELECT epoch,sequence,length(payload),length(row_digest) FROM native_workspace_v1 WHERE workspace=?1",
            params![&workspace_blob],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()
        .map_err(|error| sql(&error))?;
    let Some((epoch, sequence, payload_length, digest_length)) = metadata else {
        return Ok(None);
    };
    let workspace_lengths: [i64; 2] = (payload_length, digest_length).into();
    budget.charge(WorkUsage {
        reads: 1,
        bytes: row_bytes(&workspace_lengths)?,
        ..WorkUsage::default()
    })?;
    budget.charge(WorkUsage {
        reads: 1,
        ..WorkUsage::default()
    })?;
    let historical: Option<(i64, i64, i64)> = connection
        .query_row(
            "SELECT length(identity_digest),length(payload),length(row_digest) FROM native_view_v1 WHERE workspace=?1 AND epoch=?2 AND sequence=?3",
            params![&workspace_blob, epoch, sequence],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(|error| sql(&error))?;
    let (identity_length, view_length, view_digest_length) =
        historical.ok_or(IndexError::SnapshotUnavailable)?;
    let historical_lengths: [i64; 3] = (identity_length, view_length, view_digest_length).into();
    budget.charge(WorkUsage {
        reads: 1,
        bytes: row_bytes(&historical_lengths)?,
        ..WorkUsage::default()
    })?;
    let mut roots = connection
        .prepare(
            "SELECT length(object),length(row_digest) FROM native_view_root_v1 WHERE workspace=?1 AND epoch=?2 AND sequence=?3 ORDER BY object LIMIT 8",
        )
        .map_err(|error| sql(&error))?;
    let lengths = roots
        .query_map(params![&workspace_blob, epoch, sequence], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
        })
        .map_err(|error| sql(&error))?;
    for (index, lengths) in lengths.enumerate() {
        if index >= 7 {
            return Err(IndexError::Storage("view has too many roots".into()));
        }
        let (object_length, digest_length) = lengths.map_err(|error| sql(&error))?;
        let root_lengths: [i64; 2] = (object_length, digest_length).into();
        budget.charge(WorkUsage {
            reads: 1,
            bytes: row_bytes(&root_lengths)?,
            ..WorkUsage::default()
        })?;
    }
    drop(roots);
    // `current_view` issues the workspace query and both history/roots
    // validation queries after the metadata preflight above.
    budget.charge(WorkUsage {
        reads: 3,
        ..WorkUsage::default()
    })?;
    current_view(connection, workspace)
}

fn next_retire_candidate(
    connection: &Connection,
    workspace: WorkspaceKey,
    epoch: u64,
    after: Option<u64>,
    floor: u64,
    budget: &mut WorkBudget,
) -> Result<Option<CatalogView>, IndexError> {
    let workspace_blob = workspace_key(workspace)?;
    let after_integer = after.map(int).transpose()?.unwrap_or(-1);
    budget.charge(WorkUsage {
        reads: 1,
        ..WorkUsage::default()
    })?;
    let metadata: Option<(i64, i64, i64, i64)> = connection
        .query_row(
            "SELECT sequence,length(identity_digest),length(payload),length(row_digest) FROM native_view_v1 WHERE workspace=?1 AND epoch=?2 AND sequence>?3 AND sequence<?4 ORDER BY sequence LIMIT 1",
            params![&workspace_blob, int(epoch)?, after_integer, int(floor)?],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()
        .map_err(|error| sql(&error))?;
    let Some((sequence, identity_length, payload_length, digest_length)) = metadata else {
        return Ok(None);
    };
    if identity_length != 32 || digest_length != 32 {
        return Err(IndexError::Storage(
            "retiring view digest shape mismatch".into(),
        ));
    }
    let lengths: [i64; 3] = (identity_length, payload_length, digest_length).into();
    budget.charge(WorkUsage {
        reads: 1,
        bytes: row_bytes(&lengths)?,
        ..WorkUsage::default()
    })?;
    let digest_blob: Vec<u8> = connection
        .query_row(
            "SELECT identity_digest FROM native_view_v1 WHERE workspace=?1 AND epoch=?2 AND sequence=?3",
            params![&workspace_blob, int(epoch)?, sequence],
            |row| row.get(0),
        )
        .map_err(|error| sql(&error))?;
    let digest_bytes: [u8; 32] = digest_blob
        .try_into()
        .map_err(|_error| IndexError::Storage("retiring view identity width".into()))?;
    let identity = ViewRef {
        workspace,
        epoch,
        sequence: uint(sequence)?,
        digest: Digest(digest_bytes),
    };

    charged_view_roots_and_read(connection, identity, budget).map(Some)
}

/// Authenticate one exact historical view after charging its bounded row and
/// root ledger before the shared decoder allocates either body.
pub(super) fn charged_exact_view(
    connection: &Connection,
    identity: ViewRef,
    budget: &mut WorkBudget,
) -> Result<CatalogView, IndexError> {
    let workspace = workspace_key(identity.workspace)?;
    budget.charge(WorkUsage {
        reads: 1,
        ..WorkUsage::default()
    })?;
    let lengths: Option<(i64, i64, i64)> = connection
        .query_row(
            "SELECT length(identity_digest),length(payload),length(row_digest)
             FROM native_view_v1 WHERE workspace=?1 AND epoch=?2 AND sequence=?3",
            params![&workspace, int(identity.epoch)?, int(identity.sequence)?],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(|error| sql(&error))?;
    let lengths: [i64; 3] = lengths.ok_or(IndexError::SnapshotUnavailable)?.into();
    let [identity_length, _, digest_length] = lengths;
    if identity_length != 32 || digest_length != 32 {
        return Err(IndexError::Storage("view digest shape mismatch".into()));
    }
    budget.charge(WorkUsage {
        reads: 1,
        bytes: row_bytes(&lengths)?,
        ..WorkUsage::default()
    })?;
    charged_view_roots_and_read(connection, identity, budget)
}

fn charged_view_roots_and_read(
    connection: &Connection,
    identity: ViewRef,
    budget: &mut WorkBudget,
) -> Result<CatalogView, IndexError> {
    budget.charge(WorkUsage {
        reads: 1,
        ..WorkUsage::default()
    })?;
    let mut statement = connection
        .prepare(
            "SELECT length(object),length(row_digest) FROM native_view_root_v1 WHERE workspace=?1 AND epoch=?2 AND sequence=?3 ORDER BY object LIMIT 8",
        )
        .map_err(|error| sql(&error))?;
    let root_lengths = statement
        .query_map(
            params![
                workspace_key(identity.workspace)?,
                int(identity.epoch)?,
                int(identity.sequence)?
            ],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )
        .map_err(|error| sql(&error))?;
    for (index, lengths) in root_lengths.enumerate() {
        if index >= 7 {
            return Err(IndexError::Storage("view has too many roots".into()));
        }
        let (object_length, digest_length) = lengths.map_err(|error| sql(&error))?;
        if object_length != 32 || digest_length != 32 {
            return Err(IndexError::Storage("view root shape mismatch".into()));
        }
        let lengths: [i64; 2] = (object_length, digest_length).into();
        budget.charge(WorkUsage {
            reads: 1,
            bytes: row_bytes(&lengths)?,
            ..WorkUsage::default()
        })?;
    }
    drop(statement);
    // `read_view_row` separately loads and validates the view and root ledger.
    budget.charge(WorkUsage {
        reads: 2,
        ..WorkUsage::default()
    })?;
    read_view_row(connection, identity)
}

fn blocked_by_pin_or_work(
    connection: &Connection,
    identity: ViewRef,
    budget: &mut WorkBudget,
) -> Result<bool, IndexError> {
    let workspace_blob = workspace_key(identity.workspace)?;
    let epoch = int(identity.epoch)?;
    let sequence = int(identity.sequence)?;
    budget.charge(WorkUsage {
        reads: 2,
        ..WorkUsage::default()
    })?;
    // Expiry is intentionally absent: an OS-live reader may still hold an
    // expired pin. Session recovery or explicit release removes that row.
    let pinned: i64 = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM native_pin_v1 WHERE workspace=?1 AND epoch=?2 AND sequence=?3)",
            params![&workspace_blob, epoch, sequence],
            |row| row.get(0),
        )
        .map_err(|error| sql(&error))?;
    let work: i64 = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM native_work_v1 INDEXED BY native_work_created_view_idx_v1 WHERE workspace=?1 AND epoch=?2 AND sequence=?3)",
            params![&workspace_blob, epoch, sequence],
            |row| row.get(0),
        )
        .map_err(|error| sql(&error))?;
    Ok(pinned != 0 || work != 0)
}

fn delete_retired_view(
    connection: &Connection,
    view: &CatalogView,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    let workspace_blob = workspace_key(view.identity.workspace)?;
    let epoch = int(view.identity.epoch)?;
    let sequence = int(view.identity.sequence)?;
    let root_count = distinct_roots(view).len();
    if root_count == 0 || root_count > 7 {
        return Err(IndexError::Storage(
            "retiring view root count invalid".into(),
        ));
    }
    let writes = u64::try_from(root_count)
        .map_err(|_error| IndexError::ResourceLimited)?
        .checked_add(1)
        .ok_or(IndexError::ResourceLimited)?;
    budget.charge(WorkUsage {
        writes,
        mutations: writes,
        ..WorkUsage::default()
    })?;
    let removed_roots = connection
        .execute(
            "DELETE FROM native_view_root_v1 WHERE workspace=?1 AND epoch=?2 AND sequence=?3",
            params![&workspace_blob, epoch, sequence],
        )
        .map_err(|error| sql(&error))?;
    if removed_roots != root_count {
        return Err(IndexError::Storage(
            "view root deletion count mismatch".into(),
        ));
    }
    let removed_view = connection
        .execute(
            "DELETE FROM native_view_v1 WHERE workspace=?1 AND epoch=?2 AND sequence=?3",
            params![&workspace_blob, epoch, sequence],
        )
        .map_err(|error| sql(&error))?;
    exactly_one(removed_view, "retire exact history view")?;
    if super::reclaim::is_bound(connection)? {
        budget.charge(WorkUsage {
            reads: writes.checked_mul(10).ok_or(IndexError::ResourceLimited)?,
            writes,
            bytes: writes.checked_mul(640).ok_or(IndexError::ResourceLimited)?,
            mutations: writes,
        })?;
        for root in distinct_roots(view) {
            super::reclaim::release(connection, root)?;
        }
    }
    Ok(())
}

fn write_floor(
    connection: &Connection,
    workspace: WorkspaceKey,
    epoch: u64,
    prior: Option<u64>,
    retain_from_sequence: u64,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    if retain_from_sequence == prior.unwrap_or(0) {
        return Ok(());
    }
    let workspace_blob = workspace_key(workspace)?;
    let epoch_integer = int(epoch)?;
    let retain_integer = int(retain_from_sequence)?;
    let checksum = fields_digest(
        FLOOR_DIGEST_DOMAIN,
        &(&workspace_blob, epoch_integer, retain_integer),
    )?;
    budget.charge(WorkUsage {
        writes: 1,
        mutations: 1,
        bytes: u64::try_from(workspace_blob.len())
            .map_err(|_error| IndexError::ResourceLimited)?
            .checked_add(32 + 16)
            .ok_or(IndexError::ResourceLimited)?,
        ..WorkUsage::default()
    })?;
    let changed = if prior.is_some() {
        connection.execute(
            "UPDATE native_exact_history_floor_v1 SET retain_from_sequence=?3,row_digest=?4 WHERE workspace=?1 AND epoch=?2",
            params![&workspace_blob, epoch_integer, retain_integer, checksum],
        )
    } else {
        connection.execute(
            "INSERT INTO native_exact_history_floor_v1(workspace,epoch,retain_from_sequence,row_digest) VALUES (?1,?2,?3,?4)",
            params![&workspace_blob, epoch_integer, retain_integer, checksum],
        )
    }.map_err(|error| sql(&error))?;
    exactly_one(changed, "advance exact history floor")
}

impl CatalogExactHistoryPort for SqliteCatalog {
    fn retain_exact_history_tail(
        &self,
        workspace: WorkspaceKey,
        epoch: u64,
        sequence_window: u64,
        budget: &mut WorkBudget,
    ) -> Result<u64, IndexError> {
        let preceding = sequence_window.checked_sub(1).ok_or_else(|| {
            IndexError::InvalidInput("exact history sequence window must be positive".into())
        })?;
        let _window_integer = int(sequence_window)?;
        budget.checkpoint()?;
        let mut guard = self.lock_native_budget(budget)?;
        let tx = begin(&mut guard)?;
        let current =
            charged_current_view(&tx, workspace, budget)?.ok_or(IndexError::SnapshotUnavailable)?;
        if current.identity.epoch != epoch {
            return Err(IndexError::SnapshotUnavailable);
        }
        budget.charge(WorkUsage {
            reads: 2,
            bytes: 32,
            ..WorkUsage::default()
        })?;
        let prior = floor_row(&tx, workspace, epoch)?;
        if prior.is_some_and(|floor| floor > current.identity.sequence) {
            return Err(IndexError::Storage(
                "exact floor exceeds current view".into(),
            ));
        }
        let floor = current
            .identity
            .sequence
            .saturating_sub(preceding)
            .max(prior.unwrap_or(0));
        write_floor(&tx, workspace, epoch, prior, floor, budget)?;
        budget.checkpoint()?;
        tx.commit().map_err(|error| sql(&error))?;
        drop(guard);
        Ok(floor)
    }

    fn exact_history_floor(&self, workspace: WorkspaceKey, epoch: u64) -> Result<u64, IndexError> {
        let guard = self
            .lock()
            .map_err(|error| IndexError::Storage(error.to_string()))?;
        let current = current_view(&guard, workspace)?.ok_or(IndexError::SnapshotUnavailable)?;
        if current.identity.epoch != epoch {
            return Err(IndexError::SnapshotUnavailable);
        }
        let floor = exact_floor_for_epoch(&guard, workspace, epoch);
        drop(guard);
        floor
    }

    fn advance_exact_history_floor(
        &self,
        workspace: WorkspaceKey,
        epoch: u64,
        retain_from_sequence: u64,
        budget: &mut WorkBudget,
    ) -> Result<u64, IndexError> {
        let _retain_integer = int(retain_from_sequence)?;
        budget.checkpoint()?;
        let mut guard = self.lock_native_budget(budget)?;
        let tx = begin(&mut guard)?;
        let current =
            charged_current_view(&tx, workspace, budget)?.ok_or(IndexError::SnapshotUnavailable)?;
        if current.identity.epoch != epoch {
            return Err(IndexError::SnapshotUnavailable);
        }
        if retain_from_sequence > current.identity.sequence {
            return Err(IndexError::InvalidInput(
                "exact history floor exceeds current view".into(),
            ));
        }
        budget.charge(WorkUsage {
            reads: 2,
            bytes: 32,
            ..WorkUsage::default()
        })?;
        let prior = floor_row(&tx, workspace, epoch)?;
        let previous = prior.unwrap_or(0);
        if retain_from_sequence < previous {
            return Err(IndexError::Conflict);
        }
        write_floor(&tx, workspace, epoch, prior, retain_from_sequence, budget)?;
        budget.checkpoint()?;
        tx.commit().map_err(|error| sql(&error))?;
        drop(guard);
        Ok(retain_from_sequence)
    }

    fn retire_exact_history_page(
        &self,
        workspace: WorkspaceKey,
        epoch: u64,
        after: Option<u64>,
        limit: u32,
        budget: &mut WorkBudget,
    ) -> Result<ViewRetirementPage, IndexError> {
        if !(1..=MAX_RETIRE_VIEWS).contains(&limit) {
            return Err(IndexError::InvalidInput(
                "view retirement page limit must be in 1..=256".into(),
            ));
        }
        let _cursor_integer = after.map(int).transpose()?;
        budget.checkpoint()?;
        let mut guard = self.lock_native_budget(budget)?;
        let tx = begin(&mut guard)?;
        budget.charge(WorkUsage {
            reads: 2,
            bytes: 32,
            ..WorkUsage::default()
        })?;
        let floor = exact_floor_for_epoch(&tx, workspace, epoch)?;
        let current =
            charged_current_view(&tx, workspace, budget)?.ok_or(IndexError::SnapshotUnavailable)?;
        if floor == 0 || after.is_some_and(|sequence| sequence >= floor) {
            budget.checkpoint()?;
            tx.commit().map_err(|error| sql(&error))?;
            drop(guard);
            return Ok(ViewRetirementPage {
                after,
                examined: 0,
                retired: 0,
                blocked: 0,
                has_more: false,
            });
        }
        let mut cursor = after;
        let mut examined = 0_u32;
        let mut retired = 0_u32;
        let mut blocked = 0_u32;
        while examined < limit {
            let Some(view) = next_retire_candidate(&tx, workspace, epoch, cursor, floor, budget)?
            else {
                break;
            };
            let identity = view.identity;
            if identity == current.identity {
                return Err(IndexError::Storage(
                    "current view appears below retention floor".into(),
                ));
            }
            let protected = blocked_by_pin_or_work(&tx, identity, budget)?;
            if protected {
                blocked = blocked.checked_add(1).ok_or(IndexError::ResourceLimited)?;
            } else {
                delete_retired_view(&tx, &view, budget)?;
                retired = retired.checked_add(1).ok_or(IndexError::ResourceLimited)?;
            }
            examined = examined.checked_add(1).ok_or(IndexError::ResourceLimited)?;
            cursor = Some(identity.sequence);
        }
        // Validate page+1 before commit so corruption cannot strand a
        // partially retired page behind an apparently usable cursor.
        let has_more =
            next_retire_candidate(&tx, workspace, epoch, cursor, floor, budget)?.is_some();
        budget.checkpoint()?;
        tx.commit().map_err(|error| sql(&error))?;
        drop(guard);
        Ok(ViewRetirementPage {
            after: cursor,
            examined,
            retired,
            blocked,
            has_more,
        })
    }
}
