//! Check workspace and view-root custody for retained and current Native routes.

use quanta_index_core::index::{IndexError, MemoryLifetime, WorkBudget, WorkUsage, WorkspaceKey};
use rusqlite::{Connection, OptionalExtension as _, params};

use super::{cached_query_row, decode, sql, workspace_key};

/// Every protected Native child must have the exact registered workspace key.
/// Older files may have been written with foreign keys disabled, so a scoped
/// read of the routed workspaces cannot establish this invariant. A view root
/// must also belong to its exact parent view.
pub(super) fn require_registered_workspace_rows_v1(
    connection: &Connection,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    fn projected_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<(Option<Vec<u8>>, i64)> {
        Ok((row.get(0)?, row.get(1)?))
    }

    let _scratch = budget.reserve_memory(16_384, MemoryLifetime::Temporary)?;
    // A composite primary key orders each family by the stored workspace.
    // The continuation seeks past all rows for one key, including old epochs.
    for (first, next) in [
        (
            "SELECT CASE WHEN typeof(child.workspace)='blob' AND length(child.workspace) BETWEEN 1 AND 4096 THEN child.workspace END, \
                    EXISTS(SELECT 1 FROM native_workspace_v1 AS parent WHERE parent.workspace=child.workspace) \
               FROM native_view_v1 AS child ORDER BY child.workspace LIMIT 1",
            "SELECT CASE WHEN typeof(child.workspace)='blob' AND length(child.workspace) BETWEEN 1 AND 4096 THEN child.workspace END, \
                    EXISTS(SELECT 1 FROM native_workspace_v1 AS parent WHERE parent.workspace=child.workspace) \
               FROM native_view_v1 AS child WHERE child.workspace > ?1 ORDER BY child.workspace LIMIT 1",
        ),
        (
            "SELECT CASE WHEN typeof(child.workspace)='blob' AND length(child.workspace) BETWEEN 1 AND 4096 THEN child.workspace END, \
                    EXISTS(SELECT 1 FROM native_workspace_v1 AS parent WHERE parent.workspace=child.workspace) \
               FROM native_frontier_v1 AS child ORDER BY child.workspace LIMIT 1",
            "SELECT CASE WHEN typeof(child.workspace)='blob' AND length(child.workspace) BETWEEN 1 AND 4096 THEN child.workspace END, \
                    EXISTS(SELECT 1 FROM native_workspace_v1 AS parent WHERE parent.workspace=child.workspace) \
               FROM native_frontier_v1 AS child WHERE child.workspace > ?1 ORDER BY child.workspace LIMIT 1",
        ),
        (
            "SELECT CASE WHEN typeof(child.workspace)='blob' AND length(child.workspace) BETWEEN 1 AND 4096 THEN child.workspace END, \
                    EXISTS(SELECT 1 FROM native_workspace_v1 AS parent WHERE parent.workspace=child.workspace) \
               FROM native_source_authority_v1 AS child ORDER BY child.workspace LIMIT 1",
            "SELECT CASE WHEN typeof(child.workspace)='blob' AND length(child.workspace) BETWEEN 1 AND 4096 THEN child.workspace END, \
                    EXISTS(SELECT 1 FROM native_workspace_v1 AS parent WHERE parent.workspace=child.workspace) \
               FROM native_source_authority_v1 AS child WHERE child.workspace > ?1 ORDER BY child.workspace LIMIT 1",
        ),
    ] {
        let mut after: Option<Vec<u8>> = None;
        loop {
            budget.checkpoint()?;
            budget.charge(WorkUsage {
                reads: 2,
                ..WorkUsage::default()
            })?;
            let row = match after.as_deref() {
                Some(key) => cached_query_row(connection, next, params![key], projected_row),
                None => cached_query_row(connection, first, [], projected_row),
            }
            .optional()
            .map_err(|error| {
                if error.sqlite_error_code() == Some(rusqlite::ErrorCode::OperationInterrupted) {
                    return budget.checkpoint().err().unwrap_or_else(|| sql(&error));
                }
                sql(&error)
            })?;
            let (bytes, linked) = match row {
                None => break,
                Some((None, _)) => return Err(IndexError::DigestConflict),
                Some((Some(bytes), linked)) => (bytes, linked),
            };
            budget.charge(WorkUsage {
                bytes: u64::try_from(bytes.len()).map_err(|_| IndexError::ResourceLimited)?,
                ..WorkUsage::default()
            })?;
            let workspace: WorkspaceKey = decode(&bytes)?;
            let key = workspace_key(workspace)?;
            if key != bytes || linked != 1 {
                return Err(IndexError::DigestConflict);
            }
            after = Some(key);
        }
    }
    require_attached_view_roots_v1(connection, budget)
}

fn require_attached_view_roots_v1(
    connection: &Connection,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    // Reclamation treats these rows as roots. Check their parents in primary
    // key order, including rows no routed or retained view would enumerate.
    let mut statement = connection
        .prepare_cached(
            "SELECT CASE WHEN typeof(root.workspace)='blob'
                             AND length(root.workspace) BETWEEN 1 AND 4096
                             AND typeof(root.epoch)='integer' AND root.epoch>=0
                             AND typeof(root.sequence)='integer' AND root.sequence>=0
                        THEN EXISTS(
                            SELECT 1 FROM native_view_v1 AS parent
                            WHERE parent.workspace=root.workspace
                              AND parent.epoch=root.epoch AND parent.sequence=root.sequence
                        ) ELSE 0 END
               FROM native_view_root_v1 AS root
              ORDER BY root.workspace,root.epoch,root.sequence,root.object",
        )
        .map_err(|error| sql(&error))?;
    let mut rows = statement.query([]).map_err(|error| sql(&error))?;
    loop {
        budget.charge(WorkUsage {
            reads: 2,
            ..WorkUsage::default()
        })?;
        let Some(row) = rows.next().map_err(|error| {
            if error.sqlite_error_code() == Some(rusqlite::ErrorCode::OperationInterrupted) {
                return budget.checkpoint().err().unwrap_or_else(|| sql(&error));
            }
            sql(&error)
        })?
        else {
            return budget.checkpoint();
        };
        if !row.get::<_, bool>(0).map_err(|error| sql(&error))? {
            return Err(IndexError::DigestConflict);
        }
    }
}
