//! Durable refusal of the retained native views sharing a failed Source body.
//! A selected seed is committed before the remaining view receipts fan out.

use std::collections::{BTreeMap, btree_map::Entry};

use quanta_index_contract::index::Digest;
use quanta_index_core::index::{MemoryLease, MemoryLifetime, WorkUsage};
use rusqlite::{Connection, OptionalExtension as _, params, types::ValueRef};

use super::*;
const SINGLE_VIEW_DOMAIN: &[u8] = b"qidx.native.view-source-body-quarantine.v1\0";
const FANOUT_DOMAIN: &[u8] = b"qidx.native.view-source-body-quarantine.v2\0";
const RECEIPT_WORK_BYTES: u64 = 4096;
const NODE_WORK_BYTES: u64 = 256;

fn schema_state(catalog: &SqliteCatalog) -> u8 {
    catalog
        .native_quarantine_schema
        .load(std::sync::atomic::Ordering::Acquire)
}

fn mark_schema_current(catalog: &SqliteCatalog) {
    catalog.native_quarantine_schema.store(
        capture_format::QUARANTINE_SCHEMA_CURRENT,
        std::sync::atomic::Ordering::Release,
    );
}

fn has_transient(catalog: &SqliteCatalog) -> bool {
    !catalog
        .transient_source_body_quarantines
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .is_empty()
}

fn ensure_or_verify_schema(
    catalog: &SqliteCatalog,
    connection: &Connection,
) -> Result<(), IndexError> {
    let result = if schema_state(catalog) == capture_format::QUARANTINE_SCHEMA_ABSENT {
        capture_format::ensure_current_quarantine_schema(connection, catalog.path()).map(|_| ())
    } else {
        capture_format::require_current_quarantine_schema(connection, catalog.path())
    };
    result.map_err(|error| match error {
        CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::IngestResourceBudgetExceeded,
            ..
        } => IndexError::ResourceLimited,
        CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::CatalogBusy,
            ..
        } => IndexError::Conflict,
        error => IndexError::Storage(error.to_string()),
    })
}

#[cfg(test)]
pub(super) fn install_schema_for_test(catalog: &SqliteCatalog) -> Result<(), CoreError> {
    let mut connection = catalog.lock()?;
    let tx = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| engine_error("begin quarantine fixture schema", catalog.path(), &error))?;
    let installed = capture_format::ensure_current_quarantine_schema(&tx, catalog.path())?;
    if installed != capture_format::QUARANTINE_SCHEMA_CURRENT {
        return Err(CoreError::Storage(
            "quarantine fixture schema did not reach the current format".into(),
        ));
    }
    tx.commit().map_err(|error| {
        engine_error("commit quarantine fixture schema", catalog.path(), &error)
    })?;
    mark_schema_current(catalog);
    Ok(())
}

fn malformed() -> IndexError {
    IndexError::Storage("native view source body quarantine row mismatch".into())
}

fn fixed32(value: ValueRef<'_>) -> Option<[u8; 32]> {
    match value {
        ValueRef::Blob(bytes) => bytes.try_into().ok(),
        _ => None,
    }
}

struct Receipt {
    identity: Digest,
    body: ObjectRef,
    fanout_complete: bool,
}

struct PendingReceipt {
    workspace: Vec<u8>,
    _memory: Option<MemoryLease>,
}

fn transient_matches(
    catalog: &SqliteCatalog,
    workspace: &[u8],
    epoch: i64,
    sequence: i64,
) -> Result<bool, IndexError> {
    let workspace = decode(workspace)?;
    let epoch = uint(epoch)?;
    let sequence = uint(sequence)?;
    let rows = catalog
        .transient_source_body_quarantines
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let lower = (
        ViewRef {
            workspace,
            epoch,
            sequence,
            digest: Digest([0; 32]),
        },
        ObjectRef(Digest([0; 32])),
    );
    let upper = (
        ViewRef {
            workspace,
            epoch,
            sequence,
            digest: Digest([u8::MAX; 32]),
        },
        ObjectRef(Digest([u8::MAX; 32])),
    );
    Ok(rows.range(lower..=upper).next().is_some())
}

fn register_transient(
    catalog: &SqliteCatalog,
    pending: &BTreeMap<ViewRef, PendingReceipt>,
    body: ObjectRef,
    budget: &WorkBudget,
) -> Result<(), IndexError> {
    let mut rows = catalog
        .transient_source_body_quarantines
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let missing = pending
        .keys()
        .filter(|view| !rows.contains_key(&(**view, body)))
        .count();
    let additions_bytes = u64::try_from(missing)
        .map_err(|_conversion_error| IndexError::ResourceLimited)?
        .checked_mul(128)
        .ok_or(IndexError::ResourceLimited)?;
    let _additions_memory = if additions_bytes == 0 {
        None
    } else {
        budget.reserve_memory(additions_bytes, MemoryLifetime::Temporary)?
    };
    let mut additions = Vec::new();
    additions
        .try_reserve(missing)
        .map_err(|_allocation_error| IndexError::ResourceLimited)?;
    for view in pending.keys() {
        if rows.contains_key(&(*view, body)) {
            continue;
        }
        let memory = budget.reserve_memory(RECEIPT_WORK_BYTES, MemoryLifetime::Retained)?;
        additions.push((*view, memory));
    }
    for (view, memory) in additions {
        if rows.insert((view, body), memory).is_some() {
            return Err(IndexError::BackendViolation(
                "duplicate transient Source body quarantine".into(),
            ));
        }
    }
    Ok(())
}

fn persist_transient_incidents(
    catalog: &SqliteCatalog,
    connection: &Connection,
    budget: &mut WorkBudget,
) -> Result<bool, IndexError> {
    let rows = catalog
        .transient_source_body_quarantines
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut bodies = BTreeMap::new();
    for (_view, body) in rows.keys() {
        budget.checkpoint()?;
        if let Entry::Vacant(entry) = bodies.entry(*body) {
            let _memory =
                entry.insert(budget.reserve_memory(NODE_WORK_BYTES, MemoryLifetime::Temporary)?);
            body_incidents::seed(connection, *body, budget)?;
        }
    }
    Ok(!rows.is_empty())
}

fn clear_committed_transient_incidents(catalog: &SqliteCatalog) {
    catalog
        .transient_source_body_quarantines
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clear();
}

pub(super) fn clear_transient_body(catalog: &SqliteCatalog, body: ObjectRef) {
    let mut rows = catalog
        .transient_source_body_quarantines
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    rows.retain(|(_view, row_body), _memory| *row_body != body);
}

pub(super) fn clear_transient_view(catalog: &SqliteCatalog, view: ViewRef) {
    let mut rows = catalog
        .transient_source_body_quarantines
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    rows.retain(|(row_view, _body), _memory| *row_view != view);
}

/// Decode and authenticate the sole receipt for an exact persisted view.
fn receipt_by_key(
    connection: &Connection,
    workspace: &[u8],
    epoch: i64,
    sequence: i64,
    budget: Option<&WorkBudget>,
) -> Result<Option<Receipt>, IndexError> {
    if let Some(budget) = budget {
        budget.checkpoint()?;
    }
    let row = cached_query_row(
        connection,
        "SELECT q.identity_digest, q.object, q.row_digest, v.identity_digest
                 FROM native_view_source_body_quarantine_v1 AS q
                 LEFT JOIN native_view_v1 AS v
                   ON v.workspace=q.workspace AND v.epoch=q.epoch AND v.sequence=q.sequence
                 WHERE q.workspace=?1 AND q.epoch=?2 AND q.sequence=?3",
        params![workspace, epoch, sequence],
        |row| {
            Ok((
                fixed32(row.get_ref(0)?),
                fixed32(row.get_ref(1)?),
                fixed32(row.get_ref(2)?),
                fixed32(row.get_ref(3)?),
            ))
        },
    )
    .optional()
    .map_err(|error| sql(&error))?;
    let Some((identity, object, digest, current_identity)) = row else {
        return Ok(None);
    };
    let _memory = budget
        .map(|budget| budget.reserve_memory(RECEIPT_WORK_BYTES, MemoryLifetime::Temporary))
        .transpose()?;
    let (Some(identity), Some(object), Some(digest), Some(current_identity)) =
        (identity, object, digest, current_identity)
    else {
        return Err(malformed());
    };
    let fields = (workspace, epoch, sequence, identity, object);
    let fanout_complete = fields_digest(FANOUT_DOMAIN, &fields)? == digest;
    if current_identity != identity
        || (!fanout_complete && fields_digest(SINGLE_VIEW_DOMAIN, &fields)? != digest)
    {
        return Err(malformed());
    }
    if let Some(budget) = budget {
        budget.checkpoint()?;
    }
    Ok(Some(Receipt {
        identity: Digest(identity),
        body: ObjectRef(Digest(object)),
        fanout_complete,
    }))
}

pub(super) fn refuse_if_quarantined(
    catalog: &SqliteCatalog,
    connection: &Connection,
    workspace: &[u8],
    epoch: i64,
    sequence: i64,
    budget: Option<&WorkBudget>,
) -> Result<(), IndexError> {
    if transient_matches(catalog, workspace, epoch, sequence)? {
        return Err(IndexError::CorruptObject(
            "native exact view Source body is quarantined".into(),
        ));
    }
    if schema_state(catalog) != capture_format::QUARANTINE_SCHEMA_ABSENT
        && receipt_by_key(connection, workspace, epoch, sequence, budget)?.is_some()
    {
        return Err(IndexError::CorruptObject(
            "native exact view Source body is quarantined".into(),
        ));
    }
    Ok(())
}

fn complete_receipt(
    connection: &Connection,
    view: ViewRef,
    workspace: &[u8],
    body: ObjectRef,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    let epoch = int(view.epoch)?;
    let sequence = int(view.sequence)?;
    budget.charge(WorkUsage {
        reads: 1,
        ..WorkUsage::default()
    })?;
    // The receipt materializes a denial for the view. The incident table
    // retains every body independently, including a second body in this view.
    if let Some(receipt) = receipt_by_key(connection, workspace, epoch, sequence, Some(budget))?
        && (receipt.fanout_complete || receipt.body != body)
    {
        return Ok(());
    }
    budget.charge(WorkUsage {
        writes: 1,
        mutations: 1,
        bytes: RECEIPT_WORK_BYTES,
        ..WorkUsage::default()
    })?;
    let _memory = budget.reserve_memory(RECEIPT_WORK_BYTES, MemoryLifetime::Temporary)?;
    let object = body.0.0.as_slice();
    let digest = fields_digest(
        FANOUT_DOMAIN,
        &(workspace, epoch, sequence, view.digest.0, object),
    )?;
    let changed = cached_execute(
        connection,
        "INSERT INTO native_view_source_body_quarantine_v1
         (workspace,epoch,sequence,identity_digest,object,row_digest)
         VALUES (?1,?2,?3,?4,?5,?6)
         ON CONFLICT(workspace,epoch,sequence) DO UPDATE SET row_digest=excluded.row_digest
         WHERE identity_digest=excluded.identity_digest AND object=excluded.object",
        params![
            workspace,
            epoch,
            sequence,
            view.digest.0.as_slice(),
            object,
            digest
        ],
    )
    .map_err(|error| sql(&error))?;
    exactly_one(changed, "quarantine native exact view Source body")
}

fn quarantine_root_views(
    connection: &Connection,
    root: ObjectRef,
    selected: Option<ViewRef>,
    budget: &mut WorkBudget,
    pending: &mut BTreeMap<ViewRef, PendingReceipt>,
) -> Result<bool, IndexError> {
    let mut statement = connection
        .prepare_cached(
            "SELECT r.workspace, r.epoch, r.sequence, r.row_digest, v.identity_digest
             FROM native_view_root_v1 AS r
             LEFT JOIN native_view_v1 AS v
               ON v.workspace=r.workspace AND v.epoch=r.epoch AND v.sequence=r.sequence
             WHERE r.object=?1 ORDER BY r.workspace, r.epoch, r.sequence",
        )
        .map_err(|error| sql(&error))?;
    let object = root.0.0.as_slice();
    let mut rows = statement
        .query(params![object])
        .map_err(|error| sql(&error))?;
    let mut selected_found = false;
    let workspace_limit = usize::try_from(MAX_VIEW_PAYLOAD_BYTES)
        .map_err(|_conversion_error| IndexError::ResourceLimited)?;
    loop {
        budget.checkpoint()?;
        let Some(row) = rows.next().map_err(|error| sql(&error))? else {
            break;
        };
        budget.charge(WorkUsage {
            reads: 2,
            bytes: RECEIPT_WORK_BYTES,
            ..WorkUsage::default()
        })?;
        let _memory = budget.reserve_memory(RECEIPT_WORK_BYTES, MemoryLifetime::Temporary)?;
        let workspace = view_blob(
            row,
            0,
            1,
            workspace_limit,
            "native view root workspace mismatch",
        )?;
        let epoch = row.get(1).map_err(|error| sql(&error))?;
        let sequence = row.get(2).map_err(|error| sql(&error))?;
        let row_digest = view_blob(row, 3, 32, 32, "native view root digest shape mismatch")?;
        let identity =
            fixed32(row.get_ref(4).map_err(|error| sql(&error))?).ok_or_else(malformed)?;
        if row_digest
            != fields_digest(
                b"qidx.native.viewroot.v1",
                &(workspace, epoch, sequence, object),
            )?
        {
            return Err(IndexError::DigestConflict);
        }
        let view = ViewRef {
            workspace: decode(workspace)?,
            epoch: uint(epoch)?,
            sequence: uint(sequence)?,
            digest: Digest(identity),
        };
        selected_found |= Some(view) == selected;
        if let Entry::Vacant(entry) = pending.entry(view) {
            budget.charge(WorkUsage {
                reads: u64::try_from(VIEW_ROOT_QUERY_LIMIT)
                    .map_err(|_conversion_error| IndexError::ResourceLimited)?,
                bytes: RECEIPT_WORK_BYTES,
                ..WorkUsage::default()
            })?;
            let _view = read_view_row_by_key(connection, view, workspace)?;
            let memory = budget.reserve_memory(RECEIPT_WORK_BYTES, MemoryLifetime::Temporary)?;
            let _inserted = entry.insert(PendingReceipt {
                workspace: workspace.to_vec(),
                _memory: memory,
            });
        }
    }
    Ok(selected_found)
}

fn visit_body_ancestors(
    connection: &Connection,
    body: ObjectRef,
    visited: &mut BTreeMap<ObjectRef, Option<MemoryLease>>,
    budget: &mut WorkBudget,
    mut visit: impl FnMut(ObjectRef, &mut WorkBudget) -> Result<bool, IndexError>,
) -> Result<bool, IndexError> {
    let Entry::Vacant(seed) = visited.entry(body) else {
        return Ok(false);
    };
    let _overhead = budget.reserve_memory(RECEIPT_WORK_BYTES, MemoryLifetime::Temporary)?;
    let _seed = seed.insert(budget.reserve_memory(NODE_WORK_BYTES, MemoryLifetime::Temporary)?);
    let mut pending = vec![body];
    let mut statement = connection
        .prepare_cached(
            "SELECT parent, row_digest FROM native_object_edge_v1
             WHERE child=?1 ORDER BY parent",
        )
        .map_err(|error| sql(&error))?;
    while let Some(object) = pending.pop() {
        if visit(object, budget)? {
            return Ok(true);
        }
        budget.charge(WorkUsage {
            reads: 1,
            bytes: NODE_WORK_BYTES,
            ..WorkUsage::default()
        })?;
        let child = object.0.0.as_slice();
        let mut rows = statement
            .query(params![child])
            .map_err(|error| sql(&error))?;
        loop {
            budget.checkpoint()?;
            let Some(row) = rows.next().map_err(|error| sql(&error))? else {
                break;
            };
            budget.charge(WorkUsage {
                reads: 1,
                bytes: NODE_WORK_BYTES,
                ..WorkUsage::default()
            })?;
            let _memory = budget.reserve_memory(RECEIPT_WORK_BYTES, MemoryLifetime::Temporary)?;
            let parent = fixed32(row.get_ref(0).map_err(|error| sql(&error))?)
                .ok_or(IndexError::DigestConflict)?;
            let row_digest = view_blob(row, 1, 32, 32, "native object edge digest shape mismatch")?;
            if row_digest != fields_digest(b"qidx.native.edge.v1", &(parent, child))? {
                return Err(IndexError::DigestConflict);
            }
            let parent = ObjectRef(Digest(parent));
            if let Entry::Vacant(entry) = visited.entry(parent) {
                let _lease = entry
                    .insert(budget.reserve_memory(NODE_WORK_BYTES, MemoryLifetime::Temporary)?);
                pending.push(parent);
            }
        }
    }
    Ok(false)
}

fn collect_referencing_views(
    connection: &Connection,
    selected: Option<ViewRef>,
    body: ObjectRef,
    budget: &mut WorkBudget,
) -> Result<BTreeMap<ViewRef, PendingReceipt>, IndexError> {
    // Open/staged objects without a retained root do not create view receipts.
    let mut selected_found = selected.is_none();
    let mut pending = BTreeMap::new();
    let _stopped = visit_body_ancestors(
        connection,
        body,
        &mut BTreeMap::new(),
        budget,
        |root, budget| {
            budget.charge(WorkUsage {
                reads: 1,
                ..WorkUsage::default()
            })?;
            selected_found |=
                quarantine_root_views(connection, root, selected, budget, &mut pending)?;
            Ok(false)
        },
    )?;
    if !selected_found {
        return Err(IndexError::DigestConflict);
    }
    Ok(pending)
}

fn scan_receipts(
    connection: &Connection,
    budget: &mut WorkBudget,
    mut visit: impl FnMut(&[u8], i64, i64, Receipt, &mut WorkBudget) -> Result<(), IndexError>,
) -> Result<(), IndexError> {
    budget.charge(WorkUsage {
        reads: 1,
        ..WorkUsage::default()
    })?;
    let mut statement = connection
        .prepare_cached(
            "SELECT workspace, epoch, sequence FROM native_view_source_body_quarantine_v1
             ORDER BY workspace, epoch, sequence",
        )
        .map_err(|error| sql(&error))?;
    let mut rows = statement.query([]).map_err(|error| sql(&error))?;
    let workspace_limit = usize::try_from(MAX_VIEW_PAYLOAD_BYTES)
        .map_err(|_conversion_error| IndexError::ResourceLimited)?;
    while let Some(row) = rows.next().map_err(|error| sql(&error))? {
        budget.charge(WorkUsage {
            reads: 2,
            bytes: RECEIPT_WORK_BYTES,
            ..WorkUsage::default()
        })?;
        let workspace = view_blob(
            row,
            0,
            1,
            workspace_limit,
            "native quarantine workspace mismatch",
        )?;
        let epoch = row.get(1).map_err(|error| sql(&error))?;
        let sequence = row.get(2).map_err(|error| sql(&error))?;
        let receipt = receipt_by_key(connection, workspace, epoch, sequence, Some(budget))?
            .ok_or_else(malformed)?;
        visit(workspace, epoch, sequence, receipt, budget)?;
    }
    Ok(())
}

/// A healthy base does not authorize reattaching a quarantined dependency.
/// This is checked before the candidate root set enters the read authority.
pub(super) fn refuse_published_dependencies(
    catalog: &SqliteCatalog,
    connection: &Connection,
    base: ViewRef,
    workspace: &[u8],
    candidate: &CatalogView,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    budget.charge(WorkUsage {
        reads: 1,
        bytes: 128,
        ..WorkUsage::default()
    })?;
    refuse_if_quarantined(
        catalog,
        connection,
        workspace,
        int(base.epoch)?,
        int(base.sequence)?,
        Some(budget),
    )?;
    let _roots_memory = budget.reserve_memory(RECEIPT_WORK_BYTES, MemoryLifetime::Temporary)?;
    let roots = distinct_roots(candidate);
    let mut visited = BTreeMap::new();
    {
        let transient = catalog
            .transient_source_body_quarantines
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for ((_view, body), _memory) in transient.iter() {
            budget.charge(WorkUsage {
                reads: 1,
                bytes: NODE_WORK_BYTES,
                ..WorkUsage::default()
            })?;
            if visit_body_ancestors(
                connection,
                *body,
                &mut visited,
                budget,
                |object, _budget| Ok(roots.binary_search(&object).is_ok()),
            )? {
                return Err(IndexError::CorruptObject(
                    "native publication depends on a quarantined Source body".into(),
                ));
            }
        }
    }
    if schema_state(catalog) == capture_format::QUARANTINE_SCHEMA_ABSENT {
        return Ok(());
    }
    body_incidents::visit(connection, budget, |body, _state, budget| {
        if visit_body_ancestors(connection, body, &mut visited, budget, |object, _budget| {
            Ok(roots.binary_search(&object).is_ok())
        })? {
            return Err(IndexError::CorruptObject(
                "native publication depends on a quarantined Source body".into(),
            ));
        }
        Ok(())
    })?;
    budget.charge(WorkUsage {
        reads: 2,
        bytes: 160,
        ..WorkUsage::default()
    })?;
    if capture_format::pending_quarantine_fanout(connection, catalog.path())
        .map_err(|error| IndexError::Storage(error.to_string()))?
        .is_some_and(|pending| pending.import_legacy)
    {
        // An upgraded catalog can still contain authenticated legacy receipts
        // before its first boot has imported them into the incident table.
        scan_receipts(connection, budget, |_, _, _, receipt, budget| {
            if visit_body_ancestors(
                connection,
                receipt.body,
                &mut visited,
                budget,
                |object, _| Ok(roots.binary_search(&object).is_ok()),
            )? {
                return Err(IndexError::CorruptObject(
                    "native publication depends on a quarantined Source body".into(),
                ));
            }
            Ok(())
        })?;
    }
    Ok(())
}

pub(super) fn resume_receipt_by_key(
    catalog: &SqliteCatalog,
    connection: &Connection,
    view: ViewRef,
    workspace: &[u8],
    budget: &mut WorkBudget,
) -> Result<Option<ObjectRef>, IndexError> {
    budget.charge(WorkUsage {
        reads: 1,
        ..WorkUsage::default()
    })?;
    let receipt = receipt_by_key(
        connection,
        workspace,
        int(view.epoch)?,
        int(view.sequence)?,
        Some(budget),
    )?;
    if let Some(receipt) = &receipt
        && !receipt.fanout_complete
    {
        let pending = collect_referencing_views(connection, Some(view), receipt.body, budget)?;
        body_incidents::seed(connection, receipt.body, budget)?;
        materialize_incident(catalog, connection, &pending, receipt.body, budget)?;
        if body_incidents::state(connection, receipt.body, budget)?
            == Some(body_incidents::State::Pending)
        {
            body_incidents::complete(connection, receipt.body, budget)?;
        }
    }
    Ok(receipt.map(|receipt| receipt.body))
}

fn complete_pending_receipts(
    catalog: &SqliteCatalog,
    connection: &Connection,
    import_legacy: bool,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    if import_legacy {
        scan_receipts(
            connection,
            budget,
            |key, epoch, sequence, receipt, budget| {
                if body_incidents::state(connection, receipt.body, budget)?.is_none() {
                    let workspace = decode(key)?;
                    if workspace_key(workspace)? != key {
                        return Err(malformed());
                    }
                    let view = ViewRef {
                        workspace,
                        epoch: uint(epoch)?,
                        sequence: uint(sequence)?,
                        digest: receipt.identity,
                    };
                    let _referencing =
                        collect_referencing_views(connection, Some(view), receipt.body, budget)?;
                    body_incidents::seed(connection, receipt.body, budget)?;
                }
                Ok(())
            },
        )?;
    }
    let mut pending = Vec::new();
    body_incidents::visit(connection, budget, |body, state, budget| {
        if state == body_incidents::State::Complete {
            return Ok(());
        }
        let memory = budget.reserve_memory(NODE_WORK_BYTES, MemoryLifetime::Temporary)?;
        pending
            .try_reserve(1)
            .map_err(|_| IndexError::ResourceLimited)?;
        pending.push((body, memory));
        Ok(())
    })?;
    for (body, _memory) in pending {
        let referencing = collect_referencing_views(connection, None, body, budget)?;
        materialize_incident(catalog, connection, &referencing, body, budget)?;
        body_incidents::complete(connection, body, budget)?;
    }
    Ok(())
}

fn materialize_incident(
    catalog: &SqliteCatalog,
    connection: &Connection,
    referencing: &BTreeMap<ViewRef, PendingReceipt>,
    body: ObjectRef,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    register_transient(catalog, referencing, body, budget)?;
    for (view, receipt) in referencing {
        complete_receipt(connection, *view, &receipt.workspace, body, budget)?;
    }
    Ok(())
}

fn mark_pending(
    catalog: &SqliteCatalog,
    connection: &Connection,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    budget.charge(WorkUsage {
        reads: 2,
        bytes: 160,
        ..WorkUsage::default()
    })?;
    if capture_format::pending_quarantine_fanout(connection, catalog.path())
        .map_err(|error| IndexError::Storage(error.to_string()))?
        .is_none()
    {
        budget.charge(WorkUsage {
            reads: 2,
            writes: 1,
            mutations: 1,
            bytes: 192,
            ..WorkUsage::default()
        })?;
        let _nonce = capture_format::mark_quarantine_fanout_pending(connection, catalog.path())
            .map_err(|error| IndexError::Storage(error.to_string()))?;
    }
    Ok(())
}

pub(super) fn stage_transient_incidents(
    catalog: &SqliteCatalog,
    connection: &Connection,
    budget: &mut WorkBudget,
) -> Result<bool, IndexError> {
    let staged = persist_transient_incidents(catalog, connection, budget)?;
    if staged {
        mark_pending(catalog, connection, budget)?;
    }
    Ok(staged)
}

fn finish_pending_incidents(
    catalog: &SqliteCatalog,
    connection: &mut Connection,
    budget: &mut WorkBudget,
    stage_transient: bool,
) -> Result<(), IndexError> {
    if schema_state(catalog) == capture_format::QUARANTINE_SCHEMA_ABSENT && !has_transient(catalog)
    {
        return Ok(());
    }
    let tx = begin(connection)?;
    ensure_or_verify_schema(catalog, &tx)?;
    if stage_transient {
        let _staged = stage_transient_incidents(catalog, &tx, budget)?;
    }
    budget.charge(WorkUsage {
        reads: 2,
        bytes: 160,
        ..WorkUsage::default()
    })?;
    let pending_format = capture_format::pending_quarantine_fanout(&tx, catalog.path())
        .map_err(|error| IndexError::Storage(error.to_string()))?;
    if let Some(pending) = &pending_format {
        complete_pending_receipts(catalog, &tx, pending.import_legacy, budget)?;
        budget.charge(WorkUsage {
            writes: 1,
            mutations: 1,
            bytes: 32,
            ..WorkUsage::default()
        })?;
        capture_format::stamp_quarantine_fanout(&tx, catalog.path(), pending)
            .map_err(|error| IndexError::Storage(error.to_string()))?;
    }
    budget.checkpoint()?;
    tx.commit().map_err(|error| sql(&error))?;
    mark_schema_current(catalog);
    if pending_format.is_some() {
        clear_committed_transient_incidents(catalog);
    }
    Ok(())
}

fn quarantined_scrub_view(
    catalog: &SqliteCatalog,
    connection: &Connection,
    view: ViewRef,
    budget: &mut WorkBudget,
) -> Result<CatalogView, IndexError> {
    budget.charge(WorkUsage {
        reads: u64::try_from(VIEW_ROOT_QUERY_LIMIT).map_err(|_| IndexError::ResourceLimited)? + 2,
        bytes: RECEIPT_WORK_BYTES,
        ..WorkUsage::default()
    })?;
    let workspace = workspace_key(view.workspace)?;
    let floor = retention::exact_floor_for_epoch_by_key(connection, &workspace, view.epoch)?;
    if view.sequence < floor {
        return Err(IndexError::SnapshotUnavailable);
    }
    let selected = read_view_row_by_key(connection, view, &workspace)?;
    match refuse_if_quarantined(
        catalog,
        connection,
        &workspace,
        int(view.epoch)?,
        int(view.sequence)?,
        Some(budget),
    ) {
        Err(IndexError::CorruptObject(_)) => Ok(selected),
        Err(error) => Err(error),
        Ok(()) => Err(IndexError::SnapshotUnavailable),
    }
}

impl SqliteCatalog {
    /// Read only the owner map needed to re-prove another body fault. This
    /// does not create a query ticket or relax the view's admission fence.
    pub fn quarantined_source_owners_for_scrub(
        &self,
        view: ViewRef,
        budget: &mut WorkBudget,
    ) -> Result<quanta_index_contract::index::MapRoot, IndexError> {
        let connection = self.lock_native_budget(budget)?;
        let selected = quarantined_scrub_view(self, &connection, view, budget)?;
        ensure_roots_with_budget(&connection, &selected, Some(budget))?;
        Ok(selected.source.owners)
    }

    /// Recheck the immutable view after a maintenance read and return whether
    /// the re-proven owner body was previously quarantined.
    pub fn source_body_incident_for_scrub(
        &self,
        view: ViewRef,
        body: ObjectRef,
        budget: &mut WorkBudget,
    ) -> Result<bool, IndexError> {
        let connection = self.lock_native_budget(budget)?;
        let _selected = quarantined_scrub_view(self, &connection, view, budget)?;
        if self
            .transient_source_body_quarantines
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains_key(&(view, body))
        {
            return Ok(true);
        }
        if schema_state(self) == capture_format::QUARANTINE_SCHEMA_ABSENT {
            return Ok(false);
        }
        Ok(body_incidents::state(&connection, body, budget)?.is_some())
    }

    /// Complete all pre-fan-out receipts before this root admits read sessions.
    pub fn complete_legacy_source_body_quarantines(
        &self,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        let mut connection = self.lock_native_budget(budget)?;
        finish_pending_incidents(self, &mut connection, budget, true)
    }

    /// Return existing evidence, completing the fence if it predates fan-out.
    pub fn resume_source_body_quarantine(
        &self,
        view: ViewRef,
        budget: &mut WorkBudget,
    ) -> Result<Option<ObjectRef>, IndexError> {
        budget.charge(WorkUsage {
            reads: 1,
            bytes: RECEIPT_WORK_BYTES,
            ..WorkUsage::default()
        })?;
        let mut connection = self.lock_native_budget(budget)?;
        let tx = begin(&mut connection)?;
        let workspace = workspace_key(view.workspace)?;
        let _view = read_view_row_by_key(&tx, view, &workspace)?;
        let receipt = if schema_state(self) == capture_format::QUARANTINE_SCHEMA_ABSENT {
            None
        } else {
            receipt_by_key(
                &tx,
                &workspace,
                int(view.epoch)?,
                int(view.sequence)?,
                Some(budget),
            )?
        };
        let transient = self
            .transient_source_body_quarantines
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .keys()
            .find_map(|(affected, body)| (*affected == view).then_some(*body));
        let legacy_seed = receipt.as_ref().filter(|receipt| !receipt.fanout_complete);
        let install_schema = legacy_seed.is_some() || transient.is_some();
        if install_schema {
            ensure_or_verify_schema(self, &tx)?;
        }
        if let Some(seed) = legacy_seed {
            let _referencing = collect_referencing_views(&tx, Some(view), seed.body, budget)?;
            body_incidents::seed(&tx, seed.body, budget)?;
            mark_pending(self, &tx, budget)?;
        }
        let has_transient = stage_transient_incidents(self, &tx, budget)?;
        budget.checkpoint()?;
        tx.commit().map_err(|error| sql(&error))?;
        if install_schema {
            mark_schema_current(self);
        }
        if legacy_seed.is_some() || has_transient {
            finish_pending_incidents(self, &mut connection, budget, false)?;
        }
        Ok(receipt.map(|receipt| receipt.body).or(transient))
    }

    /// Fence all retained views referencing an independently re-proven object.
    ///
    /// The persisted v1 schema keeps its historical Source-body names, but its
    /// object graph and receipt identity are object-generic. This entry point is
    /// used for selected member roots as well as Source bodies; every incident
    /// is committed before its per-view fan-out receipts.
    pub fn quarantine_view_object(
        &self,
        view: ViewRef,
        object: ObjectRef,
        budget: &mut WorkBudget,
    ) -> Result<ObjectRef, IndexError> {
        budget.charge(WorkUsage {
            reads: 2,
            bytes: RECEIPT_WORK_BYTES,
            ..WorkUsage::default()
        })?;
        let mut connection = self.lock_native_budget(budget)?;
        let tx = begin(&mut connection)?;
        ensure_or_verify_schema(self, &tx)?;
        let workspace = workspace_key(view.workspace)?;
        let _view = read_view_row_by_key(&tx, view, &workspace)?;
        let referencing = collect_referencing_views(&tx, Some(view), object, budget)?;
        if body_incidents::state(&tx, object, budget)? != Some(body_incidents::State::Complete) {
            register_transient(self, &referencing, object, budget)?;
        }
        // Persist the object-keyed incident before attempting any per-view receipt.
        // A localized receipt write failure can then be completed on boot without
        // relying on the process-local transient fence or a maintenance retry.
        let _staged = stage_transient_incidents(self, &tx, budget)?;
        budget.checkpoint()?;
        tx.commit().map_err(|error| sql(&error))?;
        mark_schema_current(self);
        finish_pending_incidents(self, &mut connection, budget, false)?;
        Ok(object)
    }

    /// Fence all retained views referencing an independently re-proven Source body.
    /// Each proven body is durable independently of the first view receipt.
    pub fn quarantine_source_body(
        &self,
        view: ViewRef,
        body: ObjectRef,
        budget: &mut WorkBudget,
    ) -> Result<ObjectRef, IndexError> {
        self.quarantine_view_object(view, body, budget)
    }
}
