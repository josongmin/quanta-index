//! Explicit, bounded eviction of terminal work and its local custody.

use quanta_index_core::index::ports::{
    CatalogTerminalWorkRetentionPort, TerminalWorkRetirementPage, WorkRetirementCursor,
};
use quanta_index_core::index::{
    Digest, IndexError, ObjectRef, WorkBudget, WorkStatus, WorkUsage, WorkspaceKey,
};
use rusqlite::{Connection, OptionalExtension as _, params};

use super::{
    OperationKey, OperationStatus, SqliteCatalog, UpdateKind, UpdateRequest, WorkOwnership,
    WorkRow, begin, check_digest, decode, exactly_one, fields_digest, int, operation_key,
    owned_work, read_operation, read_work, retention, sql, uint, work_key, work_roots,
    workspace_key,
};

const MAX_PAGE_ROWS: u32 = 256;
// WorkScope has at most three roots; Owner work can add one prepared artifact.
const MAX_WORK_CHILDREN: usize = 4;

struct Candidate {
    cursor: WorkRetirementCursor,
    row: WorkRow,
    children: Vec<ObjectRef>,
    managed_disposition: ManagedDisposition,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ManagedDisposition {
    Blocked,
    RetainReceipt,
    DropUnusedRequest(OperationKey),
}

fn charged_bytes(lengths: &[i64]) -> Result<u64, IndexError> {
    lengths.iter().try_fold(0_u64, |total, &length| {
        total
            .checked_add(uint(length)?)
            .ok_or(IndexError::ResourceLimited)
    })
}

fn read_children(
    connection: &Connection,
    key: &[u8],
    budget: &mut WorkBudget,
) -> Result<Vec<ObjectRef>, IndexError> {
    budget.charge(WorkUsage {
        reads: 1,
        ..WorkUsage::default()
    })?;
    let mut statement = connection.prepare(
        "SELECT length(object),length(row_digest) FROM native_work_object_v1 WHERE work=?1 ORDER BY object LIMIT 5",
    ).map_err(|error| sql(&error))?;
    let lengths = statement
        .query_map(params![key], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
        })
        .map_err(|error| sql(&error))?;
    let mut count = 0_usize;
    for lengths in lengths {
        if count == MAX_WORK_CHILDREN {
            return Err(IndexError::Storage(
                "work has too many custody children".into(),
            ));
        }
        let (object_length, digest_length) = lengths.map_err(|error| sql(&error))?;
        if object_length != 32 || digest_length != 32 {
            return Err(IndexError::Storage(
                "work child custody shape mismatch".into(),
            ));
        }
        let row_lengths: [i64; 2] = (object_length, digest_length).into();
        budget.charge(WorkUsage {
            reads: 1,
            bytes: charged_bytes(&row_lengths)?,
            ..WorkUsage::default()
        })?;
        count = count.checked_add(1).ok_or(IndexError::ResourceLimited)?;
    }
    drop(statement);
    let child_bytes = u64::try_from(count)
        .map_err(|_error| IndexError::ResourceLimited)?
        .checked_mul(64)
        .ok_or(IndexError::ResourceLimited)?;
    budget.charge(WorkUsage {
        reads: 1,
        bytes: child_bytes,
        ..WorkUsage::default()
    })?;
    let mut statement = connection.prepare(
        "SELECT object,row_digest FROM native_work_object_v1 WHERE work=?1 ORDER BY object LIMIT 5",
    ).map_err(|error| sql(&error))?;
    let rows = statement
        .query_map(params![key], |row| {
            Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, Vec<u8>>(1)?))
        })
        .map_err(|error| sql(&error))?;
    let mut children = Vec::with_capacity(count);
    for row in rows {
        let (object, digest) = row.map_err(|error| sql(&error))?;
        if children.len() == MAX_WORK_CHILDREN || object.len() != 32 || digest.len() != 32 {
            return Err(IndexError::Storage("work child custody changed".into()));
        }
        if digest != fields_digest(b"qidx.native.workobject.v1", &(key, &object))? {
            return Err(IndexError::Storage(
                "work child custody digest mismatch".into(),
            ));
        }
        let identity: [u8; 32] = object
            .try_into()
            .map_err(|_error| IndexError::Storage("work child width".into()))?;
        children.push(ObjectRef(Digest(identity)));
    }
    if children.len() != count {
        return Err(IndexError::Storage(
            "work child custody count changed".into(),
        ));
    }
    Ok(children)
}

fn managed_request_count(
    connection: &Connection,
    key: &[u8],
    budget: &mut WorkBudget,
) -> Result<usize, IndexError> {
    budget.charge(WorkUsage {
        reads: 1,
        ..WorkUsage::default()
    })?;
    let mut statement = connection
        .prepare(
            "SELECT length(operation),length(work),length(payload),length(row_digest)
             FROM native_owned_request_v1 INDEXED BY native_owned_request_work_idx_v1
             WHERE work=?1 ORDER BY operation LIMIT 2",
        )
        .map_err(|error| sql(&error))?;
    let lengths = statement
        .query_map(params![key], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
            ))
        })
        .map_err(|error| sql(&error))?;
    let mut count = 0_usize;
    for lengths in lengths {
        let (operation, work, payload, digest) = lengths.map_err(|error| sql(&error))?;
        if operation <= 0
            || uint(operation)? > owned_work::MAX_OWNED_WORK_ROW_BYTES
            || work != 32
            || payload <= 0
            || uint(payload)? > owned_work::MAX_OWNED_WORK_ROW_BYTES
            || digest != 32
        {
            return Err(IndexError::Storage(
                "owned request retention shape mismatch".into(),
            ));
        }
        let row_lengths: [i64; 4] = (operation, work, payload, digest).into();
        budget.charge(WorkUsage {
            reads: 1,
            bytes: charged_bytes(&row_lengths)?,
            ..WorkUsage::default()
        })?;
        count = count.checked_add(1).ok_or(IndexError::ResourceLimited)?;
    }
    Ok(count)
}

fn managed_assignment(
    connection: &Connection,
    row: &WorkRow,
    key: &[u8],
    budget: &mut WorkBudget,
) -> Result<quanta_index_core::index::OwnedWorkAssignment, IndexError> {
    budget.charge(WorkUsage {
        reads: 1,
        ..WorkUsage::default()
    })?;
    let lengths: Option<(i64, i64, i64)> = connection
        .query_row(
            "SELECT length(workspace),length(payload),length(row_digest)
             FROM native_owned_work_v1 WHERE work=?1",
            params![key],
            |record| Ok((record.get(0)?, record.get(1)?, record.get(2)?)),
        )
        .optional()
        .map_err(|error| sql(&error))?;
    let Some((workspace, payload, digest)) = lengths else {
        return Err(IndexError::Storage(
            "managed terminal work lacks assignment".into(),
        ));
    };
    if workspace
        != i64::try_from(workspace_key(row.intent.workspace)?.len())
            .map_err(|_error| IndexError::ResourceLimited)?
        || payload <= 0
        || uint(payload)? > owned_work::MAX_OWNED_WORK_ROW_BYTES
        || digest != 32
    {
        return Err(IndexError::Storage(
            "managed assignment retention shape mismatch".into(),
        ));
    }
    let row_lengths: [i64; 3] = (workspace, payload, digest).into();
    budget.charge(WorkUsage {
        reads: 2,
        bytes: charged_bytes(&row_lengths)?,
        ..WorkUsage::default()
    })?;
    owned_work::read_owned(connection, &row.intent)?
        .ok_or_else(|| IndexError::Storage("managed terminal work assignment disappeared".into()))
}

fn managed_receipt_terminal(
    connection: &Connection,
    work: &WorkRow,
    request: &UpdateRequest,
    budget: &mut WorkBudget,
) -> Result<bool, IndexError> {
    let key = operation_key(request.operation)?;
    budget.charge(WorkUsage {
        reads: 1,
        ..WorkUsage::default()
    })?;
    let lengths: Option<(i64, i64, i64)> = connection
        .query_row(
            "SELECT length(workspace),length(payload),length(row_digest)
             FROM native_operation_v1 WHERE operation=?1",
            params![&key],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(|error| sql(&error))?;
    let Some((workspace, payload, digest)) = lengths else {
        return Ok(false);
    };
    if workspace
        != i64::try_from(workspace_key(request.operation.workspace)?.len())
            .map_err(|_error| IndexError::ResourceLimited)?
        || payload <= 0
        || uint(payload)? > owned_work::MAX_OWNED_WORK_ROW_BYTES
        || digest != 32
    {
        return Err(IndexError::Storage(
            "managed receipt retention shape mismatch".into(),
        ));
    }
    let row_lengths: [i64; 3] = (workspace, payload, digest).into();
    budget.charge(WorkUsage {
        reads: 1,
        bytes: charged_bytes(&row_lengths)?,
        ..WorkUsage::default()
    })?;
    let operation = read_operation(connection, request.operation)?
        .ok_or_else(|| IndexError::Storage("managed terminal receipt disappeared".into()))?;
    match (work.status, operation.status) {
        (WorkStatus::Completed(expected), OperationStatus::Published(actual))
            if expected == actual =>
        {
            Ok(true)
        }
        (WorkStatus::Blocked(expected), OperationStatus::Refused(actual)) if expected == actual => {
            Ok(true)
        }
        (WorkStatus::Superseded, OperationStatus::Published(_) | OperationStatus::Refused(_)) => {
            Ok(true)
        }
        (_, OperationStatus::Pending(_)) => Ok(false),
        (_, OperationStatus::Unknown) => Err(IndexError::Storage(
            "persisted unknown managed operation".into(),
        )),
        _ => Err(IndexError::Storage(
            "managed terminal work and receipt disagree".into(),
        )),
    }
}

fn managed_retirable(
    connection: &Connection,
    row: &WorkRow,
    key: &[u8],
    budget: &mut WorkBudget,
) -> Result<ManagedDisposition, IndexError> {
    if !matches!(
        row.status,
        WorkStatus::Completed(_) | WorkStatus::Superseded | WorkStatus::Blocked(_)
    ) {
        return Ok(ManagedDisposition::Blocked);
    }
    let request_count = managed_request_count(connection, key, budget)?;
    if request_count == 0 {
        return Err(IndexError::Storage(
            "managed terminal work lacks retained request".into(),
        ));
    }
    if request_count > 1 {
        return Ok(ManagedDisposition::Blocked);
    }
    let assignment = managed_assignment(connection, row, key, budget)?;
    if assignment.active {
        return Ok(ManagedDisposition::Blocked);
    }
    let UpdateKind::Materialize { claim } = &assignment.request.kind else {
        return Err(IndexError::Storage(
            "managed assignment has no materialize claim".into(),
        ));
    };
    if claim.intent != row.intent {
        return Err(IndexError::Storage(
            "managed assignment work mismatch".into(),
        ));
    }
    if assignment.dispatches == 0 {
        // No dispatched request can have a receipt or ingress. The sole
        // immutable request is removable only with its terminal assignment.
        if row.status != WorkStatus::Superseded {
            return Ok(ManagedDisposition::Blocked);
        }
        let operation = operation_key(assignment.request.operation)?;
        budget.charge(WorkUsage {
            reads: 2,
            ..WorkUsage::default()
        })?;
        let receipt: i64 = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM native_operation_v1 WHERE operation=?1)",
                params![&operation],
                |record| record.get(0),
            )
            .map_err(|error| sql(&error))?;
        let ingress: i64 = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM native_ingress_v1 WHERE operation=?1)",
                params![&operation],
                |record| record.get(0),
            )
            .map_err(|error| sql(&error))?;
        return Ok(if receipt == 0 && ingress == 0 {
            ManagedDisposition::DropUnusedRequest(assignment.request.operation)
        } else {
            ManagedDisposition::Blocked
        });
    }
    Ok(
        if managed_receipt_terminal(connection, row, &assignment.request, budget)? {
            ManagedDisposition::RetainReceipt
        } else {
            ManagedDisposition::Blocked
        },
    )
}

fn next_candidate(
    connection: &Connection,
    workspace: WorkspaceKey,
    epoch: u64,
    after: Option<WorkRetirementCursor>,
    floor: u64,
    budget: &mut WorkBudget,
) -> Result<Option<Candidate>, IndexError> {
    let workspace_blob = workspace_key(workspace)?;
    let after_sequence = after
        .map(|cursor| int(cursor.sequence))
        .transpose()?
        .unwrap_or(-1);
    let after_work = after.map_or_else(Vec::new, |cursor| cursor.work.0.to_vec());
    budget.charge(WorkUsage {
        reads: 1,
        ..WorkUsage::default()
    })?;
    let metadata: Option<(i64, i64, i64, i64, i64, i64)> = connection.query_row(
        "SELECT sequence,length(work),length(operation),length(workspace),length(payload),length(row_digest)
         FROM native_work_v1 WHERE workspace=?1 AND epoch=?2 AND sequence<?3
           AND (sequence,work)>(?4,?5) ORDER BY sequence,work LIMIT 1",
        params![&workspace_blob, int(epoch)?, int(floor)?, after_sequence, &after_work],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?)),
    ).optional().map_err(|error| sql(&error))?;
    let Some((
        sequence,
        work_length,
        operation_length,
        workspace_length,
        payload_length,
        digest_length,
    )) = metadata
    else {
        return Ok(None);
    };
    if work_length != 32
        || digest_length != 32
        || workspace_length
            != i64::try_from(workspace_blob.len()).map_err(|_error| IndexError::ResourceLimited)?
        || operation_length <= 0
        || uint(operation_length)? > super::owned_work::MAX_OWNED_WORK_ROW_BYTES
        || payload_length <= 0
        || uint(payload_length)? > super::owned_work::MAX_OWNED_WORK_ROW_BYTES
    {
        return Err(IndexError::Storage(
            "retiring work row shape mismatch".into(),
        ));
    }
    budget.charge(WorkUsage {
        reads: 1,
        bytes: charged_bytes(&[
            work_length,
            operation_length,
            workspace_length,
            payload_length,
            digest_length,
        ])?,
        ..WorkUsage::default()
    })?;
    let (key, payload, checksum): (Vec<u8>, Vec<u8>, Vec<u8>) = connection
        .query_row(
            "SELECT work,payload,row_digest FROM native_work_v1 WHERE workspace=?1 AND epoch=?2
           AND sequence<?3 AND (sequence,work)>(?4,?5) ORDER BY sequence,work LIMIT 1",
            params![
                &workspace_blob,
                int(epoch)?,
                int(floor)?,
                after_sequence,
                &after_work
            ],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(|error| sql(&error))?;
    if key.len() != 32
        || payload.len()
            != usize::try_from(payload_length).map_err(|_error| IndexError::ResourceLimited)?
        || checksum.len() != 32
    {
        return Err(IndexError::Storage("retiring work row changed".into()));
    }
    check_digest(b"qidx.native.work.v2", &payload, &checksum)?;
    let decoded: WorkRow = decode(&payload)?;
    if work_key(&decoded.intent)? != key
        || decoded.intent.workspace != workspace
        || decoded.intent.epoch != epoch
        || decoded.created_view.workspace != workspace
        || decoded.created_view.epoch != epoch
        || decoded.created_view.sequence != uint(sequence)?
    {
        return Err(IndexError::Storage(
            "retiring work identity mismatch".into(),
        ));
    }
    // The work FK binds the created view key, not its digest. Verify that
    // the retained ViewRef names the actual historical identity.
    budget.charge(WorkUsage {
        reads: 1,
        ..WorkUsage::default()
    })?;
    let view_digest_length: i64 = connection
        .query_row(
            "SELECT length(identity_digest) FROM native_view_v1 WHERE workspace=?1 AND epoch=?2 AND sequence=?3",
            params![&workspace_blob, int(epoch)?, sequence],
            |row| row.get(0),
        )
        .map_err(|error| sql(&error))?;
    if view_digest_length != 32 {
        return Err(IndexError::Storage(
            "created view identity shape mismatch".into(),
        ));
    }
    budget.charge(WorkUsage {
        reads: 1,
        bytes: 32,
        ..WorkUsage::default()
    })?;
    let view_digest: Vec<u8> = connection
        .query_row(
            "SELECT identity_digest FROM native_view_v1 WHERE workspace=?1 AND epoch=?2 AND sequence=?3",
            params![&workspace_blob, int(epoch)?, sequence],
            |row| row.get(0),
        )
        .map_err(|error| sql(&error))?;
    if view_digest.as_slice() != decoded.created_view.digest.0.as_slice() {
        return Err(IndexError::Storage(
            "created work view digest mismatch".into(),
        ));
    }
    let children = read_children(connection, &key, budget)?;
    let mut expected = work_roots(&decoded.intent);
    if let Some(prepared) = &decoded.prepared {
        expected.push(prepared.artifact);
    }
    // `read_work` verifies every declared root, including aliases, while the
    // physical child ledger contains each distinct object only once.
    let verification_reads =
        u64::try_from(expected.len()).map_err(|_error| IndexError::ResourceLimited)?;
    expected.sort_unstable();
    expected.dedup();
    if children != expected {
        return Err(IndexError::Storage(
            "work child custody ledger mismatch".into(),
        ));
    }
    let verified_child_bytes = verification_reads
        .checked_mul(32)
        .ok_or(IndexError::ResourceLimited)?;
    budget.charge(WorkUsage {
        reads: 1_u64
            .checked_add(verification_reads)
            .ok_or(IndexError::ResourceLimited)?,
        bytes: charged_bytes(&[
            operation_length,
            workspace_length,
            payload_length,
            digest_length,
        ])?
        .checked_add(verified_child_bytes)
        .ok_or(IndexError::ResourceLimited)?,
        ..WorkUsage::default()
    })?;
    let canonical = read_work(connection, &decoded.intent)?
        .ok_or_else(|| IndexError::Storage("retiring work disappeared".into()))?;
    if canonical != decoded {
        return Err(IndexError::Storage("retiring work payload changed".into()));
    }
    let work: [u8; 32] = key
        .try_into()
        .map_err(|_error| IndexError::Storage("retiring work key width".into()))?;
    let managed_disposition = if canonical.ownership == WorkOwnership::Managed {
        managed_retirable(connection, &canonical, &work, budget)?
    } else {
        ManagedDisposition::Blocked
    };
    Ok(Some(Candidate {
        cursor: WorkRetirementCursor {
            sequence: uint(sequence)?,
            work: Digest(work),
        },
        row: canonical,
        children,
        managed_disposition,
    }))
}

fn is_blocked(
    connection: &Connection,
    candidate: &Candidate,
    budget: &mut WorkBudget,
) -> Result<bool, IndexError> {
    if !matches!(
        candidate.row.status,
        WorkStatus::Completed(_) | WorkStatus::Superseded | WorkStatus::Blocked(_)
    ) {
        return Ok(true);
    }
    let key = candidate.cursor.work.0.to_vec();
    let workspace = workspace_key(candidate.row.intent.workspace)?;
    budget.charge(WorkUsage {
        reads: 1,
        ..WorkUsage::default()
    })?;
    // Pin expiry does not revoke a live OS session's read custody.
    let pin: i64 = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM native_pin_v1 WHERE workspace=?1 AND epoch=?2 AND sequence=?3)",
        params![&workspace, int(candidate.row.intent.epoch)?, int(candidate.cursor.sequence)?],
        |row| row.get(0),
    ).map_err(|error| sql(&error))?;
    if pin != 0 {
        return Ok(true);
    }
    if candidate.row.ownership == WorkOwnership::Managed {
        return Ok(matches!(
            candidate.managed_disposition,
            ManagedDisposition::Blocked
        ));
    }
    budget.charge(WorkUsage {
        reads: 2,
        ..WorkUsage::default()
    })?;
    let assignment: i64 = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM native_owned_work_v1 WHERE work=?1)",
            params![&key],
            |row| row.get(0),
        )
        .map_err(|error| sql(&error))?;
    let request: i64 = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM native_owned_request_v1 INDEXED BY native_owned_request_work_idx_v1 WHERE work=?1)",
        params![&key], |row| row.get(0),
    ).map_err(|error| sql(&error))?;
    Ok(assignment != 0 || request != 0)
}

fn delete_candidate(
    connection: &Connection,
    candidate: &Candidate,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    let key = candidate.cursor.work.0.to_vec();
    let managed = candidate.row.ownership == WorkOwnership::Managed;
    let drop_request = matches!(
        candidate.managed_disposition,
        ManagedDisposition::DropUnusedRequest(_)
    );
    let writes = u64::try_from(candidate.children.len())
        .map_err(|_error| IndexError::ResourceLimited)?
        .checked_add(1)
        .and_then(|count| count.checked_add(u64::from(managed)))
        .and_then(|count| count.checked_add(u64::from(drop_request)))
        .ok_or(IndexError::ResourceLimited)?;
    budget.charge(WorkUsage {
        writes,
        mutations: writes,
        ..WorkUsage::default()
    })?;
    if managed {
        if let ManagedDisposition::DropUnusedRequest(operation) = candidate.managed_disposition {
            let request = operation_key(operation)?;
            let removed_request = connection
                .execute(
                    "DELETE FROM native_owned_request_v1 WHERE operation=?1 AND work=?2",
                    params![&request, &key],
                )
                .map_err(|error| sql(&error))?;
            exactly_one(removed_request, "retire undispatched owned request")?;
        }
        let removed_assignment = connection
            .execute(
                "DELETE FROM native_owned_work_v1 WHERE work=?1",
                params![&key],
            )
            .map_err(|error| sql(&error))?;
        exactly_one(removed_assignment, "retire managed work assignment")?;
    }
    let removed_children = connection
        .execute(
            "DELETE FROM native_work_object_v1 WHERE work=?1",
            params![&key],
        )
        .map_err(|error| sql(&error))?;
    if removed_children != candidate.children.len() {
        return Err(IndexError::Storage(
            "work child deletion count mismatch".into(),
        ));
    }
    let removed = connection
        .execute("DELETE FROM native_work_v1 WHERE work=?1", params![&key])
        .map_err(|error| sql(&error))?;
    exactly_one(removed, "retire terminal work")?;
    if super::reclaim::is_bound(connection)? {
        let releases = u64::try_from(candidate.children.len())
            .map_err(|_error| IndexError::ResourceLimited)?;
        budget.charge(WorkUsage {
            reads: releases
                .checked_mul(10)
                .ok_or(IndexError::ResourceLimited)?,
            writes: releases,
            bytes: releases
                .checked_mul(640)
                .ok_or(IndexError::ResourceLimited)?,
            mutations: releases,
        })?;
        for child in &candidate.children {
            super::reclaim::release(connection, *child)?;
        }
    }
    Ok(())
}

impl CatalogTerminalWorkRetentionPort for SqliteCatalog {
    fn retire_terminal_work_page(
        &self,
        workspace: WorkspaceKey,
        epoch: u64,
        after: Option<WorkRetirementCursor>,
        limit: u32,
        budget: &mut WorkBudget,
    ) -> Result<TerminalWorkRetirementPage, IndexError> {
        if !(1..=MAX_PAGE_ROWS).contains(&limit) {
            return Err(IndexError::InvalidInput(
                "work retirement page limit must be in 1..=256".into(),
            ));
        }
        let _epoch = int(epoch)?;
        let _after = after.map(|cursor| int(cursor.sequence)).transpose()?;
        budget.checkpoint()?;
        let mut guard = self.lock_native_budget(budget)?;
        let tx = begin(&mut guard)?;
        budget.charge(WorkUsage {
            reads: 2,
            bytes: 32,
            ..WorkUsage::default()
        })?;
        let floor = retention::exact_floor_for_epoch(&tx, workspace, epoch)?;
        let current = retention::charged_current_view(&tx, workspace, budget)?
            .ok_or(IndexError::SnapshotUnavailable)?;
        if floor == 0 || after.is_some_and(|cursor| cursor.sequence >= floor) {
            budget.checkpoint()?;
            tx.commit().map_err(|error| sql(&error))?;
            drop(guard);
            return Ok(TerminalWorkRetirementPage {
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
            let Some(candidate) = next_candidate(&tx, workspace, epoch, cursor, floor, budget)?
            else {
                break;
            };
            if candidate.row.created_view == current.identity {
                return Err(IndexError::Storage(
                    "current view work lies below floor".into(),
                ));
            }
            if is_blocked(&tx, &candidate, budget)? {
                blocked = blocked.checked_add(1).ok_or(IndexError::ResourceLimited)?;
            } else {
                delete_candidate(&tx, &candidate, budget)?;
                retired = retired.checked_add(1).ok_or(IndexError::ResourceLimited)?;
            }
            examined = examined.checked_add(1).ok_or(IndexError::ResourceLimited)?;
            cursor = Some(candidate.cursor);
        }
        // A malformed next row aborts the whole page, not a successful prefix.
        let has_more = next_candidate(&tx, workspace, epoch, cursor, floor, budget)?.is_some();
        budget.checkpoint()?;
        tx.commit().map_err(|error| sql(&error))?;
        drop(guard);
        Ok(TerminalWorkRetirementPage {
            after: cursor,
            examined,
            retired,
            blocked,
            has_more,
        })
    }
}
