//! Durable catalog-owned work scheduling. The work row, claim, assignment and
//! original operation receipt share the catalog's one transaction authority.

use std::time::Instant;

use super::*;
use quanta_index_core::index::{
    OWNED_WORK_PRODUCER, OwnedReadyOutcome, OwnedWorkAssignment, OwnedWorkBegin, OwnedWorkDefer,
    OwnedWorkGrant, OwnedWorkPolicy, OwnedWorkReport, OwnedWorkTerminal, StreamId, UpdateKind,
    UpdateRequest, WorkBudgetDenial, WorkLimits,
};

type OwnedSqlRow = (Vec<u8>, i64, i64, i64, Vec<u8>, Vec<u8>);
// Owned request/assignment IR contains only fixed-width identities and scalars.
// Inspect the SQLite blob before copying or decoding an untrusted durable row.
const MAX_OWNED_PAYLOAD_BYTES: usize = 16 * 1024;
// WorkRow is a closed JSON shape: at most 43 fixed [u8; 16/32] leaves,
// 10 u64 fields, and fewer than 100 fixed field names. A 32-byte all-255
// JSON array is 129 bytes; 43*129 + 10*20 + 100*64 + 512 for enum wrappers
// is below 16 KiB. A schema/codec expansion must revise this bound and its
// maximal-shape test before it can make an existing row unreadable.
pub(super) const MAX_OWNED_WORK_ROW_BYTES: u64 = 16 * 1024;

fn bounded_payload(row: &rusqlite::Row<'_>, column: usize) -> rusqlite::Result<Vec<u8>> {
    let value = row.get_ref(column)?;
    let rusqlite::types::ValueRef::Blob(bytes) = value else {
        return Err(rusqlite::Error::InvalidColumnType(
            column,
            "payload".into(),
            value.data_type(),
        ));
    };
    if bytes.is_empty() || bytes.len() > MAX_OWNED_PAYLOAD_BYTES {
        return Err(rusqlite::Error::FromSqlConversionFailure(
            column,
            rusqlite::types::Type::Blob,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "owned payload exceeds its fixed IR envelope",
            )),
        ));
    }
    Ok(bytes.to_vec())
}

fn encode_owned<T: serde::Serialize>(value: &T) -> Result<Vec<u8>, IndexError> {
    let payload = encode(value)?;
    if payload.is_empty() || payload.len() > MAX_OWNED_PAYLOAD_BYTES {
        return Err(IndexError::BackendViolation(
            "owned IR exceeds its fixed envelope".into(),
        ));
    }
    Ok(payload)
}

const OWNED_OPEN_QUERY: &str = "SELECT work,length(payload),ready_since_ms FROM native_work_v1
    WHERE workspace=?1 AND epoch=?2 AND owner_mode=0 AND status=0
    ORDER BY ready_since_ms,work LIMIT ?3";
const OWNED_DUE_QUERY: &str = "SELECT native_owned_work_v1.work,length(native_owned_work_v1.payload),native_owned_work_v1.due_ms FROM native_owned_work_v1
    JOIN native_work_v1 USING(work)
    WHERE native_owned_work_v1.workspace=?1 AND native_owned_work_v1.epoch=?2
      AND native_owned_work_v1.due_ms<=?3 AND native_work_v1.status=1
      AND native_work_v1.owner_mode=2
    ORDER BY native_owned_work_v1.due_ms,native_owned_work_v1.work LIMIT ?4";

fn owned_request(
    connection: &Connection,
    key: OperationKey,
) -> Result<Option<UpdateRequest>, IndexError> {
    let row: Option<(Vec<u8>, Vec<u8>, Vec<u8>)> = connection
        .query_row(
            "SELECT work,payload,row_digest FROM native_owned_request_v1 WHERE operation=?1",
            params![operation_key(key)?],
            |row| {
                Ok((
                    bounded_payload(row, 0)?,
                    bounded_payload(row, 1)?,
                    bounded_payload(row, 2)?,
                ))
            },
        )
        .optional()
        .map_err(|error| sql(&error))?;
    let Some((work, payload, checksum)) = row else {
        return Ok(None);
    };
    check_digest(b"qidx.native.ownedrequest.v1", &payload, &checksum)?;
    let request: UpdateRequest = decode(&payload)?;
    let UpdateKind::Materialize { claim } = &request.kind else {
        return Err(IndexError::Storage(
            "owned request is not materialize".into(),
        ));
    };
    if request.operation != key || work_key(&claim.intent)? != work {
        return Err(IndexError::Storage(
            "owned request identity mismatch".into(),
        ));
    }
    Ok(Some(request))
}

fn insert_owned_request(
    connection: &Connection,
    request: &UpdateRequest,
) -> Result<(), IndexError> {
    let UpdateKind::Materialize { claim } = &request.kind else {
        return Err(IndexError::InvalidInput(
            "owned request must materialize".into(),
        ));
    };
    let payload = encode_owned(request)?;
    let inserted = connection.execute(
        "INSERT INTO native_owned_request_v1(operation,work,payload,row_digest) VALUES (?1,?2,?3,?4)",
        params![operation_key(request.operation)?, work_key(&claim.intent)?, &payload,
            digest(b"qidx.native.ownedrequest.v1", &payload).to_vec()],
    ).map_err(|error| sql(&error))?;
    exactly_one(inserted, "insert owned immutable request")
}

pub(super) fn read_owned(
    connection: &Connection,
    intent: &WorkIntent,
) -> Result<Option<OwnedWorkAssignment>, IndexError> {
    let row: Option<OwnedSqlRow> = connection.query_row(
        "SELECT workspace,epoch,due_ms,active,payload,row_digest FROM native_owned_work_v1 WHERE work=?1",
        params![work_key(intent)?],
        |row| Ok((bounded_payload(row, 0)?, row.get(1)?, row.get(2)?, row.get(3)?, bounded_payload(row, 4)?, bounded_payload(row, 5)?)),
    ).optional().map_err(|error| sql(&error))?;
    let Some((workspace, epoch, due, active, payload, checksum)) = row else {
        return Ok(None);
    };
    check_digest(b"qidx.native.ownedwork.v1", &payload, &checksum)?;
    let assignment: OwnedWorkAssignment = decode(&payload)?;
    assignment
        .policy
        .validate()
        .map_err(|error| IndexError::Storage(format!("invalid persisted owned policy: {error}")))?;
    let UpdateKind::Materialize { claim } = &assignment.request.kind else {
        return Err(IndexError::Storage(
            "owned work has no materialize claim".into(),
        ));
    };
    if claim.intent != *intent
        || assignment.request.operation.producer != OWNED_WORK_PRODUCER
        || assignment.request.operation.workspace != intent.workspace
        || assignment.request.operation.epoch != intent.epoch
        || assignment.request.expected_view.workspace != intent.workspace
        || assignment.request.expected_view.epoch != intent.epoch
        || workspace != workspace_key(intent.workspace)?
        || epoch != int(intent.epoch)?
        || due != int(assignment.due_ms)?
        || active != i64::from(assignment.active)
        || assignment.dispatches > assignment.policy.max_dispatches
        || owned_request(connection, assignment.request.operation)?
            != Some(assignment.request.clone())
    {
        return Err(IndexError::Storage(
            "owned work assignment identity mismatch".into(),
        ));
    }
    Ok(Some(assignment))
}

pub(super) fn write_owned(
    connection: &Connection,
    assignment: &OwnedWorkAssignment,
    insert: bool,
) -> Result<(), IndexError> {
    let UpdateKind::Materialize { claim } = &assignment.request.kind else {
        return Err(IndexError::InvalidInput(
            "owned work requires materialize".into(),
        ));
    };
    let key = work_key(&claim.intent)?;
    let payload = encode_owned(assignment)?;
    let checksum = digest(b"qidx.native.ownedwork.v1", &payload).to_vec();
    let changed = if insert {
        connection.execute(
            "INSERT INTO native_owned_work_v1(work,workspace,epoch,due_ms,active,payload,row_digest)
             VALUES (?1,?2,?3,?4,?5,?6,?7)",
            params![&key, workspace_key(claim.intent.workspace)?, int(claim.intent.epoch)?,
                int(assignment.due_ms)?, i64::from(assignment.active), &payload, &checksum],
        )
    } else {
        connection.execute(
            "UPDATE native_owned_work_v1 SET due_ms=?2,active=?3,payload=?4,row_digest=?5 WHERE work=?1",
            params![&key, int(assignment.due_ms)?, i64::from(assignment.active), &payload, &checksum],
        )
    }.map_err(|error| sql(&error))?;
    exactly_one(changed, "write owned work assignment")
}

pub(super) fn delete_owned(connection: &Connection, intent: &WorkIntent) -> Result<(), IndexError> {
    let removed = connection
        .execute(
            "DELETE FROM native_owned_work_v1 WHERE work=?1",
            params![work_key(intent)?],
        )
        .map_err(|error| sql(&error))?;
    at_most_one(removed, "reset owned work generation")
}

pub(super) fn park_owned(connection: &Connection, intent: &WorkIntent) -> Result<(), IndexError> {
    if let Some(mut assignment) = read_owned(connection, intent)? {
        if read_work(connection, intent)?.is_some_and(|work| work.status == WorkStatus::Superseded)
        {
            mark_pending_operation_refused(
                connection,
                assignment.request.operation,
                Refusal::FenceLost,
            )?;
        }
        assignment.active = false;
        assignment.due_ms =
            u64::try_from(i64::MAX).map_err(|_error| IndexError::ResourceLimited)?;
        write_owned(connection, &assignment, false)?;
    }
    Ok(())
}

// The caller has just written Completed in this same transaction and retains
// the assignment authenticated before that write. Superseded uses park_owned.
pub(super) fn park_completed_owned(
    connection: &Connection,
    assignment: &OwnedWorkAssignment,
) -> Result<(), IndexError> {
    let mut parked = assignment.clone();
    parked.active = false;
    parked.due_ms = u64::try_from(i64::MAX).map_err(|_error| IndexError::ResourceLimited)?;
    write_owned(connection, &parked, false)
}

fn next_request_key(
    connection: &Connection,
    intent: &WorkIntent,
) -> Result<OperationKey, IndexError> {
    let digest = work_key(intent)?;
    let bytes: [u8; 16] = digest
        .get(..16)
        .ok_or_else(|| IndexError::Storage("owned work digest width mismatch".into()))?
        .try_into()
        .map_err(|_error| IndexError::Storage("owned work stream width mismatch".into()))?;
    Ok(OperationKey {
        workspace: intent.workspace,
        epoch: intent.epoch,
        producer: OWNED_WORK_PRODUCER,
        stream: StreamId(bytes),
        stream_epoch: 1,
        sequence: next_fence(connection)?,
    })
}

fn new_request(
    connection: &Connection,
    current: ViewRef,
    claim: &WorkClaim,
) -> Result<UpdateRequest, IndexError> {
    Ok(UpdateRequest {
        operation: next_request_key(connection, &claim.intent)?,
        expected_view: current,
        kind: UpdateKind::Materialize { claim: *claim },
    })
}

fn mark_pending_operation_refused(
    connection: &Connection,
    key: OperationKey,
    reason: Refusal,
) -> Result<(), IndexError> {
    if let Some(mut operation) = read_operation(connection, key)?
        && matches!(operation.status, OperationStatus::Pending(_))
    {
        transition_claims(
            connection,
            key,
            Some(&operation.events),
            EventClaimState::Refused,
        )?;
        operation.status = OperationStatus::Refused(reason);
        write_operation(connection, &operation, false)?;
    }
    Ok(())
}

fn block(
    connection: &Connection,
    mut work: WorkRow,
    mut assignment: OwnedWorkAssignment,
    reason: Refusal,
) -> Result<(), IndexError> {
    mark_pending_operation_refused(connection, assignment.request.operation, reason)?;
    work.status = WorkStatus::Blocked(reason);
    write_work(connection, &work, false)?;
    assignment.active = false;
    assignment.due_ms = i64::MAX
        .try_into()
        .map_err(|_error| IndexError::ResourceLimited)?;
    write_owned(connection, &assignment, false)
}

fn rotate(
    connection: &Connection,
    mut work: WorkRow,
    mut assignment: OwnedWorkAssignment,
    current: ViewRef,
    now: u64,
    old_reason: Refusal,
) -> Result<Option<OwnedWorkAssignment>, IndexError> {
    mark_pending_operation_refused(connection, assignment.request.operation, old_reason)?;
    if now >= assignment.deadline_ms || assignment.dispatches >= assignment.policy.max_dispatches {
        block(connection, work, assignment, Refusal::ResourceLimited)?;
        return Ok(None);
    }
    let due_ms = now
        .checked_add(assignment.policy.retry_backoff_ms)
        .ok_or(IndexError::ResourceLimited)?;
    let lease_until_ms = due_ms
        .checked_add(assignment.policy.lease_ms)
        .ok_or(IndexError::ResourceLimited)?
        .min(assignment.deadline_ms);
    if lease_until_ms <= due_ms {
        block(connection, work, assignment, Refusal::ResourceLimited)?;
        return Ok(None);
    }
    let claim = WorkClaim {
        intent: work.intent,
        fence: next_fence(connection)?,
        lease_until_ms,
    };
    let request = new_request(connection, current, &claim)?;
    insert_owned_request(connection, &request)?;
    assignment.request = request;
    assignment.active = false;
    assignment.due_ms = due_ms;
    work.status = WorkStatus::Claimed(claim);
    write_work(connection, &work, false)?;
    write_owned(connection, &assignment, false)?;
    Ok(Some(assignment))
}

fn same_assignment_identity(left: &OwnedWorkAssignment, right: &OwnedWorkAssignment) -> bool {
    left.request == right.request
        && left.policy == right.policy
        && left.deadline_ms == right.deadline_ms
        && left.dispatches == right.dispatches
}

fn due_rows(
    connection: &Connection,
    sql_text: &str,
    workspace: WorkspaceKey,
    epoch: u64,
    now: u64,
    limit: u32,
    owned: bool,
    budget: &mut WorkBudget,
) -> Result<Vec<(u64, Vec<u8>)>, IndexError> {
    let mut statement = connection.prepare(sql_text).map_err(|error| sql(&error))?;
    let mut rows = if owned {
        statement.query(params![
            workspace_key(workspace)?,
            int(epoch)?,
            int(now)?,
            i64::from(limit)
        ])
    } else {
        statement.query(params![
            workspace_key(workspace)?,
            int(epoch)?,
            i64::from(limit)
        ])
    }
    .map_err(|error| sql(&error))?;
    let mut result = Vec::new();
    while let Some(row) = rows.next().map_err(|error| sql(&error))? {
        let key = bounded_work_blob(row, 0, 32, true).map_err(|error| sql(&error))?;
        let size: i64 = row.get(1).map_err(|error| sql(&error))?;
        let due: i64 = row.get(2).map_err(|error| sql(&error))?;
        let size = uint(size)?;
        if size > MAX_OWNED_WORK_ROW_BYTES {
            return Err(IndexError::Storage("oversized owned work row".into()));
        }
        budget.charge(WorkUsage {
            reads: 1,
            bytes: size.checked_add(64).ok_or(IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;
        result.push((uint(due)?, key));
    }
    Ok(result)
}

impl SqliteCatalog {
    /// Budget for scanning both bounded ready lanes at the requested page size.
    /// This is independent of foreground request limits.
    pub fn owned_scan_limits(limit: u32) -> Result<WorkLimits, IndexError> {
        if !(1..=256).contains(&limit) {
            return Err(IndexError::InvalidInput(
                "invalid owned scan page size".into(),
            ));
        }
        let rows = u64::from(limit)
            .checked_mul(2)
            .ok_or(IndexError::ResourceLimited)?;
        let per_row = MAX_OWNED_WORK_ROW_BYTES
            .checked_add(64)
            .ok_or(IndexError::ResourceLimited)?;
        let bytes = rows
            .checked_mul(per_row)
            .ok_or(IndexError::ResourceLimited)?;
        Ok(WorkLimits {
            reads: rows,
            writes: 0,
            bytes,
            mutations: 0,
        })
    }

    pub(super) fn ready_owned_work_impl(
        &self,
        workspace: WorkspaceKey,
        epoch: u64,
        limit: u32,
        budget: &mut WorkBudget,
    ) -> Result<OwnedReadyOutcome, IndexError> {
        if limit > 256 {
            return Err(IndexError::ResourceLimited);
        }
        // Only a deadline first observed by this fresh discovery call may be
        // retried. A numeric budget refusal or storage error remains fatal.
        let prior_denial = budget.first_denial();
        let scan = (|| -> Result<Vec<WorkIntent>, IndexError> {
            budget.checkpoint()?;
            let mut guard = self.lock_native_budget(budget)?;
            let tx = guard
                .transaction_with_behavior(TransactionBehavior::Deferred)
                .map_err(|error| sql(&error))?;
            let current = current_view(&tx, workspace)?.ok_or(IndexError::SnapshotUnavailable)?;
            if current.identity.epoch != epoch {
                return Err(IndexError::Conflict);
            }
            let now = observed_catalog_now_ms(&tx, self.clock.now_unix_ms())?;
            // Existing assignments have already started their fixed lifetime.
            // Serve due retries/expired grants before admitting new assignments;
            // an older unstarted backlog must not consume their remaining budget.
            // Each lane retains its canonical (timestamp, work identity) ordering.
            let mut rows = due_rows(
                &tx,
                OWNED_DUE_QUERY,
                workspace,
                epoch,
                now,
                limit,
                true,
                budget,
            )?;
            let remaining = limit
                .checked_sub(
                    u32::try_from(rows.len()).map_err(|_error| IndexError::ResourceLimited)?,
                )
                .ok_or(IndexError::ResourceLimited)?;
            if remaining != 0 {
                rows.extend(due_rows(
                    &tx,
                    OWNED_OPEN_QUERY,
                    workspace,
                    epoch,
                    now,
                    remaining,
                    false,
                    budget,
                )?);
            }
            let mut result = Vec::with_capacity(rows.len());
            for (_, key) in rows {
                let verified = read_work_by_ready_key(&tx, &key)?
                    .ok_or_else(|| IndexError::Storage("owned ready work disappeared".into()))?;
                if work_key(&verified.intent)? != key
                    || verified.intent.workspace != workspace
                    || verified.intent.epoch != epoch
                {
                    return Err(IndexError::Storage("owned ready scope mismatch".into()));
                }
                match verified.ownership {
                    WorkOwnership::Open if verified.status == WorkStatus::Pending => {}
                    WorkOwnership::Managed => {
                        let assigned = read_owned(&tx, &verified.intent)?.ok_or_else(|| {
                            IndexError::Storage("managed ready work lacks assignment".into())
                        })?;
                        if assigned.due_ms > now
                            || !matches!(verified.status, WorkStatus::Claimed(_))
                        {
                            return Err(IndexError::Storage("managed ready state mismatch".into()));
                        }
                    }
                    WorkOwnership::Open | WorkOwnership::Manual => {
                        return Err(IndexError::Storage("owned ready ownership mismatch".into()));
                    }
                }
                result.push(verified.intent);
            }
            tx.commit().map_err(|error| sql(&error))?;
            drop(guard);
            budget.checkpoint()?;
            Ok(result)
        })();
        match scan {
            Ok(ready) => Ok(OwnedReadyOutcome::Ready(ready)),
            Err(IndexError::ResourceLimited)
                if prior_denial.is_none()
                    && budget.first_denial() == Some(WorkBudgetDenial::Deadline) =>
            {
                Ok(OwnedReadyOutcome::RetryDeadline)
            }
            Err(error) => Err(error),
        }
    }

    pub(super) fn acquire_owned_work_impl(
        &self,
        intent: &WorkIntent,
        policy: OwnedWorkPolicy,
        budget: &WorkBudget,
    ) -> Result<Option<OwnedWorkAssignment>, IndexError> {
        let mut guard = self.lock_native_budget(budget)?;
        let tx = begin(&mut guard)?;
        let current =
            current_view(&tx, intent.workspace)?.ok_or(IndexError::SnapshotUnavailable)?;
        if current.identity.epoch != intent.epoch {
            return Err(IndexError::Conflict);
        }
        let now = catalog_now_ms(&tx, self.clock.now_unix_ms())?;
        let mut work = read_work(&tx, intent)?.ok_or(IndexError::Conflict)?;
        let result = match work.ownership {
            WorkOwnership::Manual => {
                return Err(IndexError::InvalidInput(
                    "manual work cannot be acquired by the owned pump".into(),
                ));
            }
            WorkOwnership::Managed => {
                let assignment = read_owned(&tx, intent)?.ok_or_else(|| {
                    IndexError::Storage("managed work lacks durable assignment".into())
                })?;
                match work.status {
                    WorkStatus::Completed(_) | WorkStatus::Superseded | WorkStatus::Blocked(_) => {
                        None
                    }
                    WorkStatus::Claimed(claim) => {
                        match read_operation(&tx, assignment.request.operation)?
                            .map(|row| row.status)
                        {
                            Some(OperationStatus::Refused(
                                reason @ (Refusal::Conflict | Refusal::FenceLost),
                            )) => rotate(&tx, work, assignment, current.identity, now, reason)?,
                            Some(OperationStatus::Refused(reason)) => {
                                block(&tx, work, assignment, reason)?;
                                None
                            }
                            Some(OperationStatus::Published(_)) => {
                                return Err(IndexError::Storage(
                                    "claimed owned work has published receipt".into(),
                                ));
                            }
                            Some(OperationStatus::Unknown) => {
                                return Err(IndexError::Storage(
                                    "persisted unknown operation".into(),
                                ));
                            }
                            Some(OperationStatus::Pending(_)) | None => {
                                if now >= assignment.deadline_ms {
                                    block(&tx, work, assignment, Refusal::ResourceLimited)?;
                                    None
                                } else if claim.lease_until_ms <= now {
                                    rotate(
                                        &tx,
                                        work,
                                        assignment,
                                        current.identity,
                                        now,
                                        Refusal::FenceLost,
                                    )?
                                } else {
                                    Some(assignment)
                                }
                            }
                        }
                    }
                    WorkStatus::Unknown | WorkStatus::Pending => {
                        return Err(IndexError::Storage("managed work lost claim".into()));
                    }
                }
            }
            WorkOwnership::Open => {
                policy.validate()?;
                if work.status != WorkStatus::Pending {
                    drop(tx);
                    drop(guard);
                    budget.checkpoint()?;
                    return Ok(None);
                }
                let deadline_ms = now
                    .checked_add(policy.total_timeout_ms)
                    .ok_or(IndexError::ResourceLimited)?;
                let lease_until_ms = now
                    .checked_add(policy.lease_ms)
                    .ok_or(IndexError::ResourceLimited)?
                    .min(deadline_ms);
                let claim = WorkClaim {
                    intent: *intent,
                    fence: next_fence(&tx)?,
                    lease_until_ms,
                };
                let request = new_request(&tx, current.identity, &claim)?;
                insert_owned_request(&tx, &request)?;
                let assignment = OwnedWorkAssignment {
                    request,
                    policy,
                    deadline_ms,
                    dispatches: 0,
                    due_ms: now,
                    active: false,
                };
                work.status = WorkStatus::Claimed(claim);
                work.ownership = WorkOwnership::Managed;
                write_work(&tx, &work, false)?;
                write_owned(&tx, &assignment, true)?;
                Some(assignment)
            }
        };
        budget.checkpoint()?;
        tx.commit().map_err(|error| sql(&error))?;
        drop(guard);
        budget.checkpoint()?;
        Ok(result)
    }

    pub(super) fn begin_owned_work_impl(
        &self,
        assignment: &OwnedWorkAssignment,
    ) -> Result<OwnedWorkBegin, IndexError> {
        let UpdateKind::Materialize { claim } = &assignment.request.kind else {
            return Err(IndexError::InvalidInput(
                "owned assignment kind mismatch".into(),
            ));
        };
        let mut guard = self
            .lock()
            .map_err(|error| IndexError::Storage(error.to_string()))?;
        let tx = begin(&mut guard)?;
        // Sample only after connection/transaction admission. The durable
        // remaining allowance already excludes that wait; commit and grant
        // delivery spend the monotonic deadline anchored here.
        let sampled_at = Instant::now();
        let now = catalog_now_ms(&tx, self.clock.now_unix_ms())?;
        let mut work = read_work(&tx, &claim.intent)?.ok_or(IndexError::WorkFenceLost)?;
        let mut persisted = read_owned(&tx, &claim.intent)?.ok_or(IndexError::WorkFenceLost)?;
        if work.ownership != WorkOwnership::Managed
            || persisted.request != assignment.request
            || persisted.policy != assignment.policy
            || persisted.deadline_ms != assignment.deadline_ms
        {
            return Err(IndexError::WorkFenceLost);
        }
        let current_claim = match work.status {
            WorkStatus::Claimed(current_claim) => current_claim,
            WorkStatus::Completed(view) => {
                return Ok(OwnedWorkBegin::Terminal(OwnedWorkTerminal::Completed(view)));
            }
            WorkStatus::Superseded => {
                return Ok(OwnedWorkBegin::Terminal(OwnedWorkTerminal::Superseded));
            }
            WorkStatus::Blocked(reason) => {
                return Ok(OwnedWorkBegin::Terminal(OwnedWorkTerminal::Blocked(reason)));
            }
            WorkStatus::Pending | WorkStatus::Unknown => {
                return Err(IndexError::Storage("managed work lost claim".into()));
            }
        };
        if current_claim.fence != claim.fence {
            return Err(IndexError::WorkFenceLost);
        }
        if persisted.active {
            return Ok(OwnedWorkBegin::Deferred(OwnedWorkDefer::Active));
        }
        if persisted.dispatches != assignment.dispatches {
            return Err(IndexError::WorkFenceLost);
        }
        if persisted.due_ms > now {
            return Ok(OwnedWorkBegin::Deferred(OwnedWorkDefer::NotDue));
        }
        if now >= persisted.deadline_ms || persisted.dispatches >= persisted.policy.max_dispatches {
            block(&tx, work, persisted, Refusal::ResourceLimited)?;
            tx.commit().map_err(|error| sql(&error))?;
            drop(guard);
            return Ok(OwnedWorkBegin::Terminal(OwnedWorkTerminal::Blocked(
                Refusal::ResourceLimited,
            )));
        }
        if now >= current_claim.lease_until_ms {
            return Ok(OwnedWorkBegin::Deferred(OwnedWorkDefer::LeaseExpired));
        }
        let remaining_ms = persisted
            .deadline_ms
            .checked_sub(now)
            .ok_or(IndexError::ResourceLimited)?;
        let execution_deadline = sampled_at
            .checked_add(Duration::from_millis(remaining_ms))
            .ok_or(IndexError::ResourceLimited)?;
        persisted.dispatches = persisted
            .dispatches
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        persisted.active = true;
        // Queue admission may have consumed most of the original lease. Start
        // each grant with a full lease in the same transaction as the precharge.
        let lease_until_ms = current_claim
            .lease_until_ms
            .max(
                now.checked_add(persisted.policy.lease_ms)
                    .ok_or(IndexError::ResourceLimited)?,
            )
            .min(persisted.deadline_ms);
        work.status = WorkStatus::Claimed(WorkClaim {
            lease_until_ms,
            ..current_claim
        });
        persisted.due_ms = lease_until_ms;
        write_work(&tx, &work, false)?;
        write_owned(&tx, &persisted, false)?;
        let grant = OwnedWorkGrant {
            ordinal: persisted.dispatches,
            execution_deadline,
            assignment: persisted,
        };
        tx.commit().map_err(|error| sql(&error))?;
        drop(guard);
        Ok(OwnedWorkBegin::Granted(grant))
    }

    pub(super) fn finish_owned_work_impl(
        &self,
        grant: &OwnedWorkGrant,
        report: OwnedWorkReport,
    ) -> Result<(), IndexError> {
        let UpdateKind::Materialize { claim } = &grant.assignment.request.kind else {
            return Err(IndexError::InvalidInput("owned grant kind mismatch".into()));
        };
        let mut guard = self
            .lock()
            .map_err(|error| IndexError::Storage(error.to_string()))?;
        let tx = begin(&mut guard)?;
        let (now, floor) = validated_catalog_clock_sample(&tx, self.clock.now_unix_ms())?;
        let current =
            current_view(&tx, claim.intent.workspace)?.ok_or(IndexError::SnapshotUnavailable)?;
        let work = read_work(&tx, &claim.intent)?.ok_or(IndexError::WorkFenceLost)?;
        let mut assignment = read_owned(&tx, &claim.intent)?.ok_or(IndexError::WorkFenceLost)?;
        if !same_assignment_identity(&assignment, &grant.assignment)
            || assignment.dispatches != grant.ordinal
        {
            return Err(IndexError::WorkFenceLost);
        }
        let WorkStatus::Claimed(current_claim) = work.status else {
            if let WorkStatus::Completed(view) = work.status
                && read_operation(&tx, grant.assignment.request.operation)?
                    .is_some_and(|row| row.status == OperationStatus::Published(view))
            {
                // Publication already completed and parked this work atomically.
                // An exact terminal replay observes that receipt without
                // advancing the durable clock floor.
                tx.commit().map_err(|error| sql(&error))?;
                drop(guard);
                return Ok(());
            }
            return Err(IndexError::WorkFenceLost);
        };
        let _persisted_now = persist_validated_catalog_clock_sample(&tx, now, floor)?;
        if !assignment.active {
            return Err(IndexError::WorkFenceLost);
        }
        if current_claim.fence != claim.fence {
            return Err(IndexError::WorkFenceLost);
        }
        match read_operation(&tx, assignment.request.operation)?.map(|row| row.status) {
            Some(OperationStatus::Refused(reason @ (Refusal::Conflict | Refusal::FenceLost))) => {
                // The old operation stays terminal. A lost claim does not
                // invalidate this still-current work row; rotate through the
                // same original deadline/dispatch cap as Pending expiry.
                let _rotated = rotate(&tx, work, assignment, current.identity, now, reason)?;
                tx.commit().map_err(|error| sql(&error))?;
                drop(guard);
                return Ok(());
            }
            Some(OperationStatus::Refused(reason)) => {
                block(&tx, work, assignment, reason)?;
                tx.commit().map_err(|error| sql(&error))?;
                drop(guard);
                return Ok(());
            }
            Some(OperationStatus::Published(_)) => {
                return Err(IndexError::Storage(
                    "claimed owned work has published receipt".into(),
                ));
            }
            Some(OperationStatus::Unknown) => {
                return Err(IndexError::Storage("persisted unknown operation".into()));
            }
            Some(OperationStatus::Pending(_)) | None => {}
        }
        match report {
            OwnedWorkReport::Reconcile
            | OwnedWorkReport::Interrupted
            | OwnedWorkReport::Failed(Refusal::Conflict) => {
                assignment.active = false;
                assignment.due_ms = now
                    .checked_add(assignment.policy.retry_backoff_ms)
                    .ok_or(IndexError::ResourceLimited)?
                    .min(assignment.deadline_ms);
                write_owned(&tx, &assignment, false)?;
            }
            OwnedWorkReport::Failed(reason) => block(&tx, work, assignment, reason)?,
        }
        tx.commit().map_err(|error| sql(&error))?;
        drop(guard);
        Ok(())
    }

    pub(super) fn validate_owned_work_impl(
        &self,
        request: &UpdateRequest,
    ) -> Result<OperationStatus, IndexError> {
        if request.operation.producer != OWNED_WORK_PRODUCER {
            return Err(IndexError::InvalidInput(
                "request is not catalog-owned".into(),
            ));
        }
        let mut guard = self
            .lock()
            .map_err(|error| IndexError::Storage(error.to_string()))?;
        let tx = begin(&mut guard)?;
        let now = catalog_now_ms(&tx, self.clock.now_unix_ms())?;
        let (validated, status) = validate_request_state_with_status(&tx, request, now)?;
        if matches!(
            status,
            OperationStatus::Unknown | OperationStatus::Pending(_)
        ) {
            match (&validated, &request.kind) {
                (Some((work, assignment)), UpdateKind::Materialize { claim }) => {
                    validate_live_claim_row(work, claim, now)?;
                    validate_owned_live_claim(assignment, claim, now)?;
                    if assignment.request.operation != request.operation {
                        return Err(IndexError::WorkFenceLost);
                    }
                }
                // A terminal receipt paired with a live status is not an
                // expected stable transaction state. Keep the original
                // claim-validation path and its error priority in that case.
                _ => validate_materialization_claim(&tx, request, now)?,
            }
        }
        tx.commit().map_err(|error| sql(&error))?;
        drop(guard);
        Ok(status)
    }
}

pub(super) fn validate_request(
    connection: &Connection,
    request: &UpdateRequest,
    now: u64,
) -> Result<(), IndexError> {
    validate_request_state(connection, request, now).map(|_state| ())
}

pub(super) fn validate_request_state(
    connection: &Connection,
    request: &UpdateRequest,
    now: u64,
) -> Result<Option<(WorkRow, OwnedWorkAssignment)>, IndexError> {
    validate_request_state_with_status(connection, request, now).map(|(state, _status)| state)
}

fn validate_request_state_with_status(
    connection: &Connection,
    request: &UpdateRequest,
    now: u64,
) -> Result<(Option<(WorkRow, OwnedWorkAssignment)>, OperationStatus), IndexError> {
    validate_retained_request(connection, request)?;
    let status = read_operation(connection, request.operation)?
        .map_or(OperationStatus::Unknown, |row| row.status);
    if matches!(
        &status,
        OperationStatus::Published(_) | OperationStatus::Refused(_)
    ) {
        return Ok((None, status));
    }
    let claim = materialization_claim(request)?;
    let work = read_work(connection, &claim.intent)?.ok_or(IndexError::WorkFenceLost)?;
    let assignment = read_owned(connection, &claim.intent)?.ok_or(IndexError::WorkFenceLost)?;
    validate_live_request_state(request, claim, now, &work, &assignment)?;
    Ok((Some((work, assignment)), status))
}

pub(super) fn validate_request_from_live(
    connection: &Connection,
    request: &UpdateRequest,
    now: u64,
    work: &WorkRow,
    assignment: &OwnedWorkAssignment,
) -> Result<(), IndexError> {
    validate_retained_request(connection, request)?;
    if terminal_operation(connection, request)? {
        return Ok(());
    }
    let claim = materialization_claim(request)?;
    validate_live_request_state(request, claim, now, work, assignment)
}

fn validate_retained_request(
    connection: &Connection,
    request: &UpdateRequest,
) -> Result<(), IndexError> {
    let retained =
        owned_request(connection, request.operation)?.ok_or(IndexError::WorkFenceLost)?;
    if retained != *request {
        return Err(IndexError::DigestConflict);
    }
    Ok(())
}

fn terminal_operation(
    connection: &Connection,
    request: &UpdateRequest,
) -> Result<bool, IndexError> {
    Ok(
        read_operation(connection, request.operation)?.is_some_and(|row| {
            matches!(
                row.status,
                OperationStatus::Published(_) | OperationStatus::Refused(_)
            )
        }),
    )
}

fn materialization_claim(request: &UpdateRequest) -> Result<&WorkClaim, IndexError> {
    let UpdateKind::Materialize { claim } = &request.kind else {
        return Err(IndexError::InvalidInput(
            "owned request is not materialize".into(),
        ));
    };
    Ok(claim)
}

fn validate_live_request_state(
    request: &UpdateRequest,
    claim: &WorkClaim,
    now: u64,
    work: &WorkRow,
    assignment: &OwnedWorkAssignment,
) -> Result<(), IndexError> {
    let WorkStatus::Claimed(current_claim) = work.status else {
        return Err(IndexError::WorkFenceLost);
    };
    if work.ownership != WorkOwnership::Managed
        || assignment.request != *request
        || !assignment.active
        || assignment.dispatches == 0
        || current_claim.fence != claim.fence
        || now >= current_claim.lease_until_ms
        || now >= assignment.deadline_ms
    {
        return Err(IndexError::WorkFenceLost);
    }
    Ok(())
}

pub(super) fn validate_reserved_key(
    connection: &Connection,
    key: OperationKey,
    now: u64,
) -> Result<(), IndexError> {
    if key.producer != OWNED_WORK_PRODUCER {
        return Err(IndexError::InvalidInput(
            "operation producer is not reserved".into(),
        ));
    }
    let request = owned_request(connection, key)?.ok_or(IndexError::WorkFenceLost)?;
    validate_request(connection, &request, now)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error;

    macro_rules! check {
        ($condition:expr $(,)?) => {{
            if !$condition {
                return Err(format!("assertion failed: {}", stringify!($condition)).into());
            }
        }};
    }

    macro_rules! check_eq {
        ($left:expr, $right:expr $(,)?) => {{
            match (&$left, &$right) {
                (left, right) if !(*left == *right) => {
                    return Err(format!(
                        "assertion failed: left == right; left: {left:?}, right: {right:?}"
                    )
                    .into());
                }
                _ => {}
            }
        }};
    }

    #[test]
    fn owned_durable_blob_refuses_wrong_type_empty_and_oversized_before_decode()
    -> Result<(), Box<dyn Error>> {
        let db = Connection::open_in_memory()?;
        for sql_text in [
            "SELECT NULL",
            "SELECT 'text'",
            "SELECT zeroblob(0)",
            "SELECT zeroblob(16385)",
        ] {
            let result = db.query_row(sql_text, [], |row| bounded_payload(row, 0));
            check!(result.is_err());
        }
        let bytes = db.query_row("SELECT zeroblob(16384)", [], |row| bounded_payload(row, 0))?;
        check_eq!(bytes.len(), 16384);
        Ok(())
    }
}
