//! Point-in-time Catalog observation for a held read and logical retirement.
//! No empty cursor or GC queue page is treated as a debt-zero certificate.

use quanta_index_core::index::{
    CatalogReadSettlementPort, Digest, IndexError, NativeReadReleaseObservation,
    NativeReadSettlement, NativeReadSettlementScope, NativeReadSettlementStatus, ObjectRef,
    ReadSessionLease, ReadTicketLease, WorkBudget, WorkUsage,
};
use rusqlite::{Connection, OptionalExtension as _, TransactionBehavior, params};

use super::{
    SqliteCatalog, begin, exactly_one, int, lifetime, reclaim, retention, sql, workspace_key,
};

fn charged_exists(
    connection: &Connection,
    query: &'static str,
    bindings: impl rusqlite::Params,
    budget: &mut WorkBudget,
) -> Result<bool, IndexError> {
    budget.charge(WorkUsage {
        reads: 1,
        ..WorkUsage::default()
    })?;
    let exists: i64 = connection
        .query_row(query, bindings, |row| row.get(0))
        .map_err(|error| sql(&error))?;
    match exists {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(IndexError::Storage(
            "read settlement EXISTS shape changed".into(),
        )),
    }
}

/// A missing lifecycle row is not an empty reclaim queue.
///
/// Both physical registration tables are traversed by primary-key pages in the
/// same Catalog snapshot; each point lookup uses the lifecycle primary key.
pub(super) fn verify_object_lifecycle_coverage(
    connection: &Connection,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    for (first_page_query, next_page_query, expected_kind) in [
        (
            "SELECT CASE WHEN typeof(object)='blob' AND length(object)=32 THEN object END
             FROM native_object_v1 ORDER BY object LIMIT 256",
            "SELECT CASE WHEN typeof(object)='blob' AND length(object)=32 THEN object END
             FROM native_object_v1 WHERE object>?1 ORDER BY object LIMIT 256",
            1_i64,
        ),
        (
            "SELECT CASE WHEN typeof(object)='blob' AND length(object)=32 THEN object END
             FROM native_object_install_v1 ORDER BY object LIMIT 256",
            "SELECT CASE WHEN typeof(object)='blob' AND length(object)=32 THEN object END
             FROM native_object_install_v1 WHERE object>?1 ORDER BY object LIMIT 256",
            0_i64,
        ),
    ] {
        let mut after: Option<[u8; 32]> = None;
        loop {
            budget.charge(WorkUsage {
                reads: 1,
                ..WorkUsage::default()
            })?;
            let mut statement = connection
                .prepare(if after.is_some() {
                    next_page_query
                } else {
                    first_page_query
                })
                .map_err(|error| sql(&error))?;
            let mut rows = match after.as_ref() {
                Some(key) => statement.query(params![&key[..]]),
                None => statement.query([]),
            }
            .map_err(|error| sql(&error))?;
            let mut found = false;
            loop {
                budget.checkpoint()?;
                let Some(row) = rows.next().map_err(|error| sql(&error))? else {
                    break;
                };
                budget.charge(WorkUsage {
                    reads: 2,
                    bytes: 32,
                    ..WorkUsage::default()
                })?;
                let key: Option<Vec<u8>> = row.get(0).map_err(|error| sql(&error))?;
                let key: [u8; 32] = key
                    .ok_or_else(|| {
                        IndexError::Storage("stored object reference shape changed".into())
                    })?
                    .try_into()
                    .map_err(|_error| {
                        IndexError::Storage("stored object reference shape changed".into())
                    })?;
                let kind: Option<i64> = connection
                    .query_row(
                        "SELECT kind FROM native_gc_object_v1 WHERE object=?1",
                        params![&key[..]],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(|error| sql(&error))?;
                if kind != Some(expected_kind) {
                    return Err(IndexError::Storage(
                        "stored object has no matching lifecycle".into(),
                    ));
                }
                after = Some(key);
                found = true;
            }
            if !found {
                break;
            }
        }
    }
    Ok(())
}

impl CatalogReadSettlementPort for SqliteCatalog {
    fn release_held_ticket(
        &self,
        session: &dyn ReadSessionLease,
        ticket: ReadTicketLease,
        scope: NativeReadSettlementScope,
        budget: &mut WorkBudget,
    ) -> Result<NativeReadReleaseObservation, IndexError> {
        scope.validate()?;
        budget.checkpoint()?;
        let concrete = lifetime::guard(self, session)?;
        if ticket.session != concrete.binding {
            return Err(IndexError::SnapshotUnavailable);
        }
        let guard_inode = lifetime::inode(&concrete.file)?;
        let mut connection = self.lock_native_budget(budget)?;
        budget.charge(WorkUsage {
            reads: 4,
            writes: 1,
            ..WorkUsage::default()
        })?;
        let transaction = begin(&mut connection)?;
        let session_row = lifetime::checked_session(&transaction, concrete.binding)?;
        if (session_row.2, session_row.3) != guard_inode {
            return Err(IndexError::Storage("read session inode mismatch".into()));
        }
        let pin = lifetime::checked_pin(&transaction, ticket)?;
        if pin.5 != workspace_key(scope.workspace)?
            || pin.6 != int(scope.epoch)?
            || pin.7 != int(scope.target.sequence)?
        {
            return Err(IndexError::SnapshotUnavailable);
        }
        let _view = retention::charged_exact_view(&transaction, scope.target, budget)?;
        let changed = transaction
            .execute(
                "DELETE FROM native_pin_v1 WHERE ticket=?1 AND fence=?2",
                params![pin.0, pin.3],
            )
            .map_err(|error| sql(&error))?;
        exactly_one(changed, "release held read pin")?;
        let has_remaining_target_pins: i64 = transaction
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM native_pin_v1
                 WHERE workspace=?1 AND epoch=?2 AND sequence=?3)",
                params![&pin.5, pin.6, pin.7],
                |row| row.get(0),
            )
            .map_err(|error| sql(&error))?;
        let has_remaining_target_pins = match has_remaining_target_pins {
            0 => false,
            1 => true,
            _ => {
                return Err(IndexError::Storage(
                    "target pin EXISTS shape changed".into(),
                ));
            }
        };
        budget.checkpoint()?;
        transaction.commit().map_err(|error| sql(&error))?;
        drop(connection);
        Ok(NativeReadReleaseObservation {
            scope,
            has_remaining_target_pins,
        })
    }

    fn observe_read_settlement(
        &self,
        session: &dyn ReadSessionLease,
        scope: NativeReadSettlementScope,
        budget: &mut WorkBudget,
    ) -> Result<NativeReadSettlement, IndexError> {
        scope.validate()?;
        budget.checkpoint()?;
        let concrete = lifetime::guard(self, session)?;
        let guard_inode = lifetime::inode(&concrete.file)?;
        let mut connection = self.lock_native_budget(budget)?;
        // A DEFERRED read transaction fixes one SQLite snapshot without taking
        // the write reservation used for Catalog mutations.
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Deferred)
            .map_err(|error| sql(&error))?;
        // Session authentication reads one bounded row. The floor helper
        // reads its digest and (when present) the epoch head. Prepay their
        // maximum work before either helper performs SQL or copies blobs.
        budget.charge(WorkUsage {
            reads: 3,
            bytes: 80,
            ..WorkUsage::default()
        })?;
        let session_row = lifetime::checked_session(&transaction, concrete.binding)?;
        if (session_row.2, session_row.3) != guard_inode {
            return Err(IndexError::Storage("read session inode mismatch".into()));
        }
        let head = retention::charged_current_view(&transaction, scope.workspace, budget)?
            .ok_or(IndexError::SnapshotUnavailable)?
            .identity;
        if head.epoch != scope.epoch {
            return Err(IndexError::SnapshotUnavailable);
        }
        let exact_floor =
            retention::exact_floor_for_epoch(&transaction, scope.workspace, scope.epoch)?;
        let workspace = workspace_key(scope.workspace)?;
        let epoch = int(scope.epoch)?;
        let sequence = int(scope.target.sequence)?;
        let has_remaining_target_pins = charged_exists(
            &transaction,
            "SELECT EXISTS(SELECT 1 FROM native_pin_v1
             WHERE workspace=?1 AND epoch=?2 AND sequence=?3)",
            params![&workspace, epoch, sequence],
            budget,
        )?;
        let result = |status| NativeReadSettlement {
            scope,
            head,
            exact_floor,
            has_remaining_target_pins,
            status,
        };

        if has_remaining_target_pins || exact_floor <= scope.target.sequence {
            return Ok(result(NativeReadSettlementStatus::Pending));
        }
        // A work row below the floor conservatively blocks settlement. Terminal
        // work retires independently of the floor, including work on a newer
        // retained view.
        if charged_exists(
            &transaction,
            "SELECT EXISTS(SELECT 1 FROM native_work_v1
             WHERE workspace=?1 AND epoch=?2 AND sequence<?3)",
            params![&workspace, epoch, int(exact_floor)?],
            budget,
        )? || charged_exists(
            &transaction,
            "SELECT EXISTS(SELECT 1 FROM native_work_v1
             WHERE workspace=?1 AND epoch=?2 AND status IN (2,3,4))",
            params![&workspace, epoch],
            budget,
        )? || charged_exists(
            &transaction,
            "SELECT EXISTS(SELECT 1 FROM native_terminal_custody_v1
             WHERE workspace=?1 AND epoch=?2)",
            params![&workspace, epoch],
            budget,
        )? {
            return Ok(result(NativeReadSettlementStatus::Pending));
        }
        // Examine view identities in fixed pages. A nested anti-join could
        // silently scan an unbounded number of protected views in one query.
        let mut after_sequence = -1_i64;
        loop {
            budget.charge(WorkUsage {
                reads: 1,
                ..WorkUsage::default()
            })?;
            let mut statement = transaction
                .prepare(
                    "SELECT sequence FROM native_view_v1
                     WHERE workspace=?1 AND epoch=?2 AND sequence>?3 AND sequence<?4
                     ORDER BY sequence LIMIT 256",
                )
                .map_err(|error| sql(&error))?;
            let mut rows = statement
                .query(params![
                    &workspace,
                    epoch,
                    after_sequence,
                    int(exact_floor)?
                ])
                .map_err(|error| sql(&error))?;
            let mut found = false;
            loop {
                budget.checkpoint()?;
                let Some(row) = rows.next().map_err(|error| sql(&error))? else {
                    break;
                };
                budget.charge(WorkUsage {
                    reads: 1,
                    ..WorkUsage::default()
                })?;
                let sequence: i64 = row.get(0).map_err(|error| sql(&error))?;
                if charged_exists(
                    &transaction,
                    "SELECT EXISTS(SELECT 1 FROM native_pin_v1
                     WHERE workspace=?1 AND epoch=?2 AND sequence=?3)",
                    params![&workspace, epoch, sequence],
                    budget,
                )? {
                    after_sequence = sequence;
                    found = true;
                    continue;
                }
                return Ok(result(NativeReadSettlementStatus::Pending));
            }
            if !found {
                break;
            }
        }
        // A second session may still own a live pin or need death recovery.
        // OS-lock classification is not possible inside this Catalog snapshot.
        if charged_exists(
            &transaction,
            "SELECT EXISTS(SELECT 1 FROM native_read_session_open_v1)",
            [],
            budget,
        )? || charged_exists(
            &transaction,
            "SELECT EXISTS(SELECT 1 FROM native_read_session_retire_v1)",
            [],
            budget,
        )? || charged_exists(
            &transaction,
            "SELECT EXISTS(SELECT 1 FROM native_read_session_v1 WHERE id<>?1)",
            params![session_row.0],
            budget,
        )? {
            return Ok(result(NativeReadSettlementStatus::Incomplete));
        }
        if charged_exists(
            &transaction,
            "SELECT EXISTS(SELECT 1 FROM native_gc_queue_v1)",
            [],
            budget,
        )? {
            return Ok(result(NativeReadSettlementStatus::Pending));
        }

        verify_object_lifecycle_coverage(&transaction, budget)?;

        // The durable queue does not cover an unqueued LIVE orphan. Walk the
        // object index in bounded pages and use the reclaim owner's complete
        // reference/selected-boot predicate for each object. The caller's work
        // budget and deadline bound the total scan; exhaustion cannot report 0.
        let mut after: Option<[u8; 32]> = None;
        loop {
            budget.charge(WorkUsage {
                reads: 1,
                ..WorkUsage::default()
            })?;
            let query = if after.is_some() {
                "SELECT CASE WHEN typeof(object)='blob' AND length(object)=32 THEN object END,
                        phase FROM native_gc_object_v1
                 WHERE object>?1 ORDER BY object LIMIT 256"
            } else {
                "SELECT CASE WHEN typeof(object)='blob' AND length(object)=32 THEN object END,
                        phase FROM native_gc_object_v1 ORDER BY object LIMIT 256"
            };
            let mut statement = transaction.prepare(query).map_err(|error| sql(&error))?;
            let mut rows = match after.as_ref() {
                Some(key) => statement.query(params![&key[..]]),
                None => statement.query([]),
            }
            .map_err(|error| sql(&error))?;
            let mut found = false;
            loop {
                budget.checkpoint()?;
                let Some(row) = rows.next().map_err(|error| sql(&error))? else {
                    break;
                };
                budget.charge(WorkUsage {
                    reads: 1,
                    bytes: 32,
                    ..WorkUsage::default()
                })?;
                let key: Option<Vec<u8>> = row.get(0).map_err(|error| sql(&error))?;
                let key: [u8; 32] = key
                    .ok_or_else(|| IndexError::Storage("GC object reference shape changed".into()))?
                    .try_into()
                    .map_err(|_error| {
                        IndexError::Storage("GC object reference shape changed".into())
                    })?;
                let phase: i64 = row.get(1).map_err(|error| sql(&error))?;
                if phase != 0 {
                    return Ok(result(NativeReadSettlementStatus::Pending));
                }
                if !reclaim::checked_live_object(&transaction, ObjectRef(Digest(key)), budget)? {
                    return Ok(result(NativeReadSettlementStatus::Pending));
                }
                if !reclaim::reference_exists(&transaction, ObjectRef(Digest(key)), budget)? {
                    return Ok(result(NativeReadSettlementStatus::Pending));
                }
                after = Some(key);
                found = true;
            }
            if !found {
                break;
            }
        }
        budget.checkpoint()?;
        transaction.commit().map_err(|error| sql(&error))?;
        drop(connection);
        Ok(result(NativeReadSettlementStatus::SettledAtSnapshot))
    }
}
