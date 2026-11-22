//! Check that selected BOOT workspaces and their retained rows are settled.

use quanta_index_contract::index::NativeProtectedSourceRoutesV1;
use quanta_index_core::index::{
    CatalogView, ImmutableOperation, IndexError, IngressRef, MemoryLifetime, OperationStatus,
    ViewRef, WorkBudget, WorkUsage, WorkspaceKey,
};
use rusqlite::{Connection, params, types::ValueRef};

use super::{
    WorkLookup,
    boot_record::{
        SCAN_NATIVE_OPERATION_KEYS, decode_scanned_operation_key, with_quiescent_operation_v1,
    },
    decode, operation_key, owned_work, read_work_by_key, retention, sql, uint,
    workspace_integrity::require_registered_workspace_rows_v1,
    workspace_key,
};

pub(super) fn check_quiescent_rows(
    connection: &Connection,
    workspace: WorkspaceKey,
    maximum: ViewRef,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    check_quiescent_rows_with_publications(connection, workspace, maximum, budget, |_, _, _| Ok(()))
}

#[cfg(test)]
pub(super) fn check_quiescent_protected_routes(
    connection: &Connection,
    primary: ViewRef,
    routes: &NativeProtectedSourceRoutesV1,
    allowed_ingress: Option<IngressRef>,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    check_quiescent_protected_routes_with_publications(
        connection,
        primary,
        routes,
        allowed_ingress,
        budget,
        |_, _, _| Ok(()),
    )
}

pub(super) fn check_quiescent_protected_routes_with_publications(
    connection: &Connection,
    primary: ViewRef,
    routes: &NativeProtectedSourceRoutesV1,
    allowed_ingress: Option<IngressRef>,
    budget: &mut WorkBudget,
    visit_published: impl FnMut(
        ImmutableOperation,
        CatalogView,
        &mut WorkBudget,
    ) -> Result<(), IndexError>,
) -> Result<(), IndexError> {
    routes.validate().map_err(|_| IndexError::DigestConflict)?;
    check_quiescent_rows_with_publications_impl(
        connection,
        primary.workspace,
        primary,
        Some(routes),
        allowed_ingress,
        budget,
        visit_published,
    )
}

pub(super) fn check_quiescent_rows_with_publications(
    connection: &Connection,
    workspace: WorkspaceKey,
    maximum: ViewRef,
    budget: &mut WorkBudget,
    mut visit_published: impl FnMut(
        ImmutableOperation,
        CatalogView,
        &mut WorkBudget,
    ) -> Result<(), IndexError>,
) -> Result<(), IndexError> {
    check_quiescent_rows_with_publications_impl(
        connection,
        workspace,
        maximum,
        None,
        None,
        budget,
        &mut visit_published,
    )
}

fn check_quiescent_rows_with_publications_impl(
    connection: &Connection,
    workspace: WorkspaceKey,
    maximum: ViewRef,
    routes: Option<&NativeProtectedSourceRoutesV1>,
    allowed_ingress: Option<IngressRef>,
    budget: &mut WorkBudget,
    mut visit_published: impl FnMut(
        ImmutableOperation,
        CatalogView,
        &mut WorkBudget,
    ) -> Result<(), IndexError>,
) -> Result<(), IndexError> {
    let workspace_key = workspace_key(workspace)?;
    if routes.is_none() {
        let blocked: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM native_workspace_v1 WHERE workspace<>?1 LIMIT 1)
                 OR EXISTS(SELECT 1 FROM native_ingress_v1 LIMIT 1)",
                params![&workspace_key],
                |row| row.get(0),
            )
            .map_err(|error| sql(&error))?;
        if blocked {
            return Err(IndexError::Conflict);
        }
        // A single selected workspace still has to reject orphan child/root
        // rows: reclamation treats every stored view-root row as live, even
        // when no routed publication reader would enumerate its parent.
        // Finish this scan before the operation-row scratch loan begins, after
        // the existing cheap ingress/foreign-workspace refusal.
        require_registered_workspace_rows_v1(connection, budget)?;
    }
    let scratch_bytes = owned_work::MAX_OWNED_WORK_ROW_BYTES
        .checked_add(32_768)
        .ok_or(IndexError::ResourceLimited)?;
    let _scratch = budget.reserve_memory(scratch_bytes, MemoryLifetime::Temporary)?;
    let mut _ceilings_memory = None;
    let mut ceilings = Vec::<&ViewRef>::new();
    if let Some(routes) = routes {
        let ingress_blocked: bool = match allowed_ingress {
            Some(allowed) => {
                let expected = operation_key(allowed.operation)?;
                connection
                    .query_row(
                        "SELECT NOT EXISTS(SELECT 1 FROM native_ingress_v1 WHERE operation=?1 LIMIT 1)
                                OR EXISTS(SELECT 1 FROM native_ingress_v1 WHERE operation<>?1 LIMIT 1)",
                        params![&expected],
                        |row| row.get(0),
                    )
                    .map_err(|error| sql(&error))?
            }
            None => connection
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM native_ingress_v1 LIMIT 1)",
                    [],
                    |row| row.get(0),
                )
                .map_err(|error| sql(&error))?,
        };
        if ingress_blocked {
            return Err(IndexError::Conflict);
        }
        let count = routes
            .heads
            .len()
            .checked_add(routes.repomap_heads.len())
            .ok_or(IndexError::ResourceLimited)?;
        let bytes = count
            .checked_mul(std::mem::size_of::<&ViewRef>())
            .ok_or(IndexError::ResourceLimited)?;
        let bytes = u64::try_from(bytes).map_err(|_width| IndexError::ResourceLimited)?;
        _ceilings_memory = budget.reserve_memory(bytes, MemoryLifetime::Temporary)?;
        ceilings
            .try_reserve_exact(count)
            .map_err(|_allocation| IndexError::ResourceLimited)?;
        ceilings.extend(routes.heads.iter().map(|head| &head.publication.identity));
        ceilings.extend(
            routes
                .repomap_heads
                .iter()
                .map(|head| &head.publication.identity),
        );
        ceilings.sort_unstable_by_key(|head| head.workspace);
        if ceilings
            .windows(2)
            .any(|pair| matches!(pair, [left, right] if left.workspace == right.workspace && left != right))
        {
            return Err(IndexError::DigestConflict);
        }
        ceilings.dedup_by_key(|head| head.workspace);
    }
    let routed_ceiling = |key| -> Option<ViewRef> {
        ceilings
            .binary_search_by_key(&key, |head| head.workspace)
            .map_or(None, |index| ceilings.get(index).map(|head| **head))
    };
    let ceiling_for = |key| routed_ceiling(key).or_else(|| (key == workspace).then_some(maximum));
    if routes.is_some() {
        require_registered_workspace_rows_v1(connection, budget)?;
        let mut statement = connection
            .prepare(
                "SELECT CASE WHEN typeof(workspace)='blob' AND length(workspace) BETWEEN 1 AND 4096
                      THEN workspace END FROM native_workspace_v1 ORDER BY workspace",
            )
            .map_err(|error| sql(&error))?;
        let mut rows = statement.query([]).map_err(|error| sql(&error))?;
        let mut seen = 0usize;
        while let Some(row) = rows.next().map_err(|error| sql(&error))? {
            budget.charge(WorkUsage {
                reads: 1,
                bytes: 4096,
                ..WorkUsage::default()
            })?;
            let ValueRef::Blob(bytes) = row.get_ref(0).map_err(|error| sql(&error))? else {
                return Err(IndexError::DigestConflict);
            };
            if bytes.is_empty() || bytes.len() > 4096 {
                return Err(IndexError::DigestConflict);
            }
            let key: WorkspaceKey = decode(bytes)?;
            if ceiling_for(key).is_none() {
                return Err(IndexError::Conflict);
            }
            seen = seen.checked_add(1).ok_or(IndexError::ResourceLimited)?;
        }
        let primary_is_routed = routed_ceiling(workspace).is_some();
        let expected = ceilings
            .len()
            .checked_add(usize::from(!primary_is_routed))
            .ok_or(IndexError::ResourceLimited)?;
        if seen != expected {
            return Err(IndexError::Conflict);
        }
    }
    retention::verify_source_floors(connection, |key| ceiling_for(key).is_some(), budget)?;

    // Validate canonical rows, not just denormalized status columns. Charge
    // each row and its bounded lookup before decoding any owned payload.
    let mut statement = connection
        .prepare(SCAN_NATIVE_OPERATION_KEYS)
        .map_err(|error| sql(&error))?;
    let mut rows = statement.query([]).map_err(|error| sql(&error))?;
    loop {
        budget.charge(WorkUsage {
            reads: 1,
            ..WorkUsage::default()
        })?;
        let Some(row) = rows.next().map_err(|error| sql(&error))? else {
            break;
        };
        let ValueRef::Blob(key) = row.get_ref(0).map_err(|error| sql(&error))? else {
            return Err(IndexError::Storage(
                "rollback operation key is not a blob".into(),
            ));
        };
        if key.is_empty() || key.len() > 4096 {
            return Err(IndexError::Storage(
                "rollback operation key is out of bounds".into(),
            ));
        }
        budget.charge(WorkUsage {
            bytes: 4096,
            ..WorkUsage::default()
        })?;
        let key = decode_scanned_operation_key(key)?;
        with_quiescent_operation_v1(connection, key, routes, budget, |operation, budget| {
            let ceiling = ceiling_for(key.workspace).ok_or(IndexError::Conflict)?;
            match &operation.status {
                OperationStatus::Published(view) if settled_view_within_head(*view, ceiling) => {
                    let publication = operation
                        .published_view
                        .as_ref()
                        .ok_or(IndexError::SnapshotUnavailable)?;
                    visit_published(operation.immutable, publication.clone(), budget)
                }
                OperationStatus::Refused(_) => Ok(()),
                OperationStatus::Unknown
                | OperationStatus::Pending(_)
                | OperationStatus::Published(_) => Err(IndexError::Conflict),
            }
        })?;
    }
    drop(rows);
    drop(statement);
    let mut statement = connection
        .prepare(
            "SELECT CASE
                      WHEN typeof(work) != 'blob' THEN 0
                      WHEN length(work) NOT BETWEEN 1 AND 4096 THEN X''
                      ELSE work END,
                    length(payload) FROM native_work_v1 ORDER BY work",
        )
        .map_err(|error| sql(&error))?;
    let mut rows = statement.query([]).map_err(|error| sql(&error))?;
    loop {
        budget.charge(WorkUsage {
            reads: 1,
            ..WorkUsage::default()
        })?;
        let Some(row) = rows.next().map_err(|error| sql(&error))? else {
            break;
        };
        let ValueRef::Blob(key) = row.get_ref(0).map_err(|error| sql(&error))? else {
            return Err(IndexError::Storage(
                "rollback work key is not a blob".into(),
            ));
        };
        let payload = uint(row.get(1).map_err(|error| sql(&error))?)?;
        if key.is_empty()
            || key.len() > 4096
            || payload == 0
            || payload > owned_work::MAX_OWNED_WORK_ROW_BYTES
        {
            return Err(IndexError::Storage(
                "rollback work row is out of bounds".into(),
            ));
        }
        budget.charge(WorkUsage {
            reads: 2,
            bytes: payload
                .checked_add(4096)
                .ok_or(IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;
        let work = read_work_by_key(connection, key, WorkLookup::ExactKey)?
            .ok_or(IndexError::SnapshotUnavailable)?;
        let Some(ceiling) = ceiling_for(work.intent.workspace) else {
            return Err(IndexError::Conflict);
        };
        match work.status {
            quanta_index_core::index::WorkStatus::Completed(view)
                if settled_view_within_head(view, ceiling) => {}
            quanta_index_core::index::WorkStatus::Superseded => {}
            _ => return Err(IndexError::Conflict),
        }
    }
    budget.checkpoint()
}

fn settled_view_within_head(view: ViewRef, head: ViewRef) -> bool {
    view.workspace == head.workspace
        && (view == head || (view.epoch, view.sequence) < (head.epoch, head.sequence))
}
