//! The disposable confirmed-occurrence ledger is exact and attempt-local.

use super::*;
use quanta_index_core::index::{
    AttemptId, AttemptRef, CollectionId, Digest, OperationKey, ProducerId, SlotId, StreamId,
    TenantId, WorkspaceKey, WorktreeId,
};

fn attempt() -> AttemptRef {
    AttemptRef {
        operation: OperationKey {
            workspace: WorkspaceKey {
                tenant: TenantId([1; 16]),
                collection: CollectionId([2; 16]),
                worktree: WorktreeId([3; 16]),
                slot: SlotId([4; 16]),
            },
            epoch: 1,
            producer: ProducerId([5; 16]),
            stream: StreamId([6; 16]),
            stream_epoch: 1,
            sequence: 1,
        },
        attempt: AttemptId([7; 16]),
        fence: 1,
    }
}

fn reference(byte: u8) -> ObjectRef {
    ObjectRef(Digest([byte; 32]))
}

#[test]
fn confirmed_spill_keeps_all_seventy_exact_references_after_hot_window_fills()
-> Result<(), IndexError> {
    let mut work = budget()?;
    let spool = TextSpool::new(&TestScratch, &mut work)?;
    let owner = attempt();
    let references: Vec<ObjectRef> = (1_u8..=70).map(reference).collect();
    for packet in references.chunks(32) {
        spool.admit_confirmed_occurrences(packet, owner, &mut work)?;
        for reference in packet {
            check!(!spool.confirmed_occurrence_contains(*reference, owner, &mut work)?);
        }
        spool.record_confirmed_occurrences(packet, owner, &work)?;
    }
    for reference in &references {
        check!(spool.confirmed_occurrence_contains(*reference, owner, &mut work)?);
        check_eq!(
            spool.confirmed_occurrence_count(*reference, owner, &mut work)?,
            ConfirmedOccurrenceCount::Unvalidated
        );
    }
    check!(!spool.confirmed_occurrence_contains(reference(71), owner, &mut work)?);
    check_eq!(
        spool.confirmed_occurrence_count(reference(71), owner, &mut work)?,
        ConfirmedOccurrenceCount::Missing
    );
    let other = TextSpool::new(&TestScratch, &mut work)?;
    check!(!other.confirmed_occurrence_contains(reference(1), owner, &mut work)?);
    Ok(())
}

#[test]
fn confirmed_spill_refuses_foreign_attempt_and_original_budget_denials() -> Result<(), IndexError> {
    let mut work = budget()?;
    let spool = TextSpool::new(&TestScratch, &mut work)?;
    let owner = attempt();
    let foreign = AttemptRef { fence: 2, ..owner };
    let confirmed_ref = reference(81);
    spool.admit_confirmed_occurrences(&[confirmed_ref], owner, &mut work)?;
    check!(!spool.confirmed_occurrence_contains(confirmed_ref, owner, &mut work)?);
    spool.record_confirmed_occurrences(&[confirmed_ref], owner, &work)?;
    check!(spool.confirmed_occurrence_contains(confirmed_ref, owner, &mut work)?);
    check!(matches!(
        spool.confirmed_occurrence_contains(confirmed_ref, foreign, &mut work),
        Err(IndexError::FenceLost)
    ));
    check!(matches!(
        spool.confirmed_occurrence_count(confirmed_ref, foreign, &mut work),
        Err(IndexError::FenceLost)
    ));
    check!(matches!(
        spool.record_validated_occurrence_count(confirmed_ref, foreign, 1, &mut work),
        Err(IndexError::FenceLost)
    ));
    check!(matches!(
        spool.admit_confirmed_occurrences(&[reference(82)], foreign, &mut work),
        Err(IndexError::FenceLost)
    ));
    check!(matches!(
        spool.record_confirmed_occurrences(&[reference(82)], foreign, &work),
        Err(IndexError::FenceLost)
    ));
    let mut denied = WorkBudget::new(
        WorkLimits {
            reads: 0,
            writes: 0,
            bytes: 0,
            mutations: 0,
        },
        Instant::now()
            .checked_add(Duration::from_secs(10))
            .ok_or(IndexError::ResourceLimited)?,
    );
    check!(matches!(
        spool.confirmed_occurrence_contains(confirmed_ref, owner, &mut denied),
        Err(IndexError::ResourceLimited)
    ));
    check!(matches!(
        spool.confirmed_occurrence_count(confirmed_ref, owner, &mut denied),
        Err(IndexError::ResourceLimited)
    ));
    check!(matches!(
        spool.record_validated_occurrence_count(confirmed_ref, owner, 1, &mut denied),
        Err(IndexError::ResourceLimited)
    ));
    check!(matches!(
        spool.admit_confirmed_occurrences(&[reference(82)], owner, &mut denied),
        Err(IndexError::ResourceLimited)
    ));
    let mut write_denied = WorkBudget::new(
        WorkLimits {
            reads: 10,
            writes: 0,
            bytes: 1_000,
            mutations: 10,
        },
        Instant::now()
            .checked_add(Duration::from_secs(10))
            .ok_or(IndexError::ResourceLimited)?,
    );
    check!(matches!(
        spool.record_validated_occurrence_count(confirmed_ref, owner, 1, &mut write_denied),
        Err(IndexError::ResourceLimited)
    ));
    check_eq!(
        spool.confirmed_occurrence_count(confirmed_ref, owner, &mut work)?,
        ConfirmedOccurrenceCount::Unvalidated
    );
    check!(!spool.confirmed_occurrence_contains(reference(82), owner, &mut work)?);
    work.request().cancel_handle().cancel();
    check!(matches!(
        spool.confirmed_occurrence_contains(confirmed_ref, owner, &mut work),
        Err(IndexError::Cancelled)
    ));
    check!(matches!(
        spool.confirmed_occurrence_count(confirmed_ref, owner, &mut work),
        Err(IndexError::Cancelled)
    ));
    check!(matches!(
        spool.record_validated_occurrence_count(confirmed_ref, owner, 1, &mut work),
        Err(IndexError::Cancelled)
    ));
    check!(matches!(
        spool.admit_confirmed_occurrences(&[reference(82)], owner, &mut work),
        Err(IndexError::Cancelled)
    ));
    check!(matches!(
        spool.record_confirmed_occurrences(&[reference(82)], owner, &work),
        Err(IndexError::Cancelled)
    ));
    Ok(())
}

#[test]
fn confirmed_spill_sql_refusal_cannot_create_a_positive_hit() -> Result<(), IndexError> {
    let mut work = budget()?;
    let spool = TextSpool::new(&TestScratch, &mut work)?;
    let owner = attempt();
    let candidate = reference(83);
    spool.admit_confirmed_occurrences(&[candidate], owner, &mut work)?;
    check!(!spool.confirmed_occurrence_contains(candidate, owner, &mut work)?);
    spool
        .db
        .scoped(&work)?
        .execute_batch(
            "CREATE TRIGGER refuse_confirmed BEFORE INSERT ON confirmed_occurrence
             BEGIN SELECT RAISE(ABORT, 'injected confirmed-row refusal'); END;",
        )
        .map_err(|error| sql(&error))?;
    check!(matches!(
        spool.record_confirmed_occurrences(&[candidate], owner, &work),
        Err(IndexError::Storage(_))
    ));
    check!(!spool.confirmed_occurrence_contains(candidate, owner, &mut work)?);
    Ok(())
}

#[test]
fn confirmed_count_requires_one_existing_unvalidated_row_and_exact_attempt()
-> Result<(), IndexError> {
    let owner = attempt();
    let candidate = reference(84);
    {
        let mut work = budget()?;
        let spool = TextSpool::new(&TestScratch, &mut work)?;
        check_eq!(
            spool.confirmed_occurrence_count(candidate, owner, &mut work)?,
            ConfirmedOccurrenceCount::Missing
        );
        check!(matches!(
            spool.record_validated_occurrence_count(candidate, owner, 2, &mut work),
            Err(IndexError::BackendViolation(_))
        ));
    }
    for duplicate in [2, 3] {
        let mut work = budget()?;
        let spool = TextSpool::new(&TestScratch, &mut work)?;
        spool.admit_confirmed_occurrences(&[candidate], owner, &mut work)?;
        spool.record_confirmed_occurrences(&[candidate], owner, &work)?;
        check_eq!(
            spool.confirmed_occurrence_count(candidate, owner, &mut work)?,
            ConfirmedOccurrenceCount::Unvalidated
        );
        let before = work.usage();
        spool.record_validated_occurrence_count(candidate, owner, 2, &mut work)?;
        check_eq!(
            work.usage().writes,
            before
                .writes
                .checked_add(1)
                .ok_or(IndexError::ResourceLimited)?
        );
        check_eq!(
            work.usage().bytes,
            before
                .bytes
                .checked_add(128)
                .ok_or(IndexError::ResourceLimited)?
        );
        let before = work.usage();
        check_eq!(
            spool.confirmed_occurrence_count(candidate, owner, &mut work)?,
            ConfirmedOccurrenceCount::Validated(2)
        );
        check_eq!(
            work.usage().reads,
            before
                .reads
                .checked_add(1)
                .ok_or(IndexError::ResourceLimited)?
        );
        check_eq!(
            work.usage().bytes,
            before
                .bytes
                .checked_add(64)
                .ok_or(IndexError::ResourceLimited)?
        );
        check!(spool.confirmed_occurrence_contains(candidate, owner, &mut work)?);
        check!(matches!(
            spool.record_validated_occurrence_count(candidate, owner, duplicate, &mut work),
            Err(IndexError::BackendViolation(_))
        ));
        // A failed duplicate/conflicting write has no success prefix. Do not
        // retry this failed scratch attempt through its old budget.
    }
    Ok(())
}

#[test]
fn confirmed_count_rejects_malformed_sql_values_and_out_of_bound_setters() -> Result<(), IndexError>
{
    let owner = attempt();
    let candidate = reference(85);
    for count in [
        0,
        crate::native::text::MAX_POSITIONS_PER_TERM
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?,
    ] {
        let mut work = budget()?;
        let spool = TextSpool::new(&TestScratch, &mut work)?;
        spool.admit_confirmed_occurrences(&[candidate], owner, &mut work)?;
        spool.record_confirmed_occurrences(&[candidate], owner, &work)?;
        check!(matches!(
            spool.record_validated_occurrence_count(candidate, owner, count, &mut work),
            Err(IndexError::CorruptObject(_))
        ));
        check_eq!(
            spool.confirmed_occurrence_count(candidate, owner, &mut work)?,
            ConfirmedOccurrenceCount::Unvalidated
        );
    }
    let key = reference_bytes(&candidate)?;
    let over = i64::try_from(crate::native::text::MAX_POSITIONS_PER_TERM)
        .map_err(|_error| IndexError::ResourceLimited)?
        .checked_add(1)
        .ok_or(IndexError::ResourceLimited)?;
    for malformed in [
        rusqlite::types::Value::Text("wrong type".into()),
        rusqlite::types::Value::Integer(-1),
        rusqlite::types::Value::Integer(0),
        rusqlite::types::Value::Integer(over),
    ] {
        let mut work = budget()?;
        let spool = TextSpool::new(&TestScratch, &mut work)?;
        spool.admit_confirmed_occurrences(&[candidate], owner, &mut work)?;
        spool.record_confirmed_occurrences(&[candidate], owner, &work)?;
        let changed = spool
            .db
            .scoped(&work)?
            .execute(
                "UPDATE confirmed_occurrence SET validated_count=?2 WHERE ref=?1",
                params![key, malformed],
            )
            .map_err(|error| sql(&error))?;
        check_eq!(changed, 1);
        check!(matches!(
            spool.confirmed_occurrence_count(candidate, owner, &mut work),
            Err(IndexError::CorruptObject(_))
        ));
        check!(matches!(
            spool.confirmed_occurrence_contains(candidate, owner, &mut work),
            Err(IndexError::CorruptObject(_))
        ));
    }
    Ok(())
}

#[test]
fn confirmed_count_update_refusal_leaves_no_positive_count_and_releases_scratch()
-> Result<(), IndexError> {
    let mut work = budget()?;
    let spool = TextSpool::new(&TestScratch, &mut work)?;
    let path = spool.allocation.path().to_path_buf();
    let owner = attempt();
    let candidate = reference(86);
    spool.admit_confirmed_occurrences(&[candidate], owner, &mut work)?;
    spool.record_confirmed_occurrences(&[candidate], owner, &work)?;
    spool
        .db
        .scoped(&work)?
        .execute_batch(
            "CREATE TRIGGER refuse_count BEFORE UPDATE OF validated_count ON confirmed_occurrence
             BEGIN SELECT RAISE(ABORT, 'injected count refusal'); END;",
        )
        .map_err(|error| sql(&error))?;
    check!(matches!(
        spool.record_validated_occurrence_count(candidate, owner, 2, &mut work),
        Err(IndexError::Storage(_))
    ));
    check_eq!(
        spool.confirmed_occurrence_count(candidate, owner, &mut work)?,
        ConfirmedOccurrenceCount::Unvalidated
    );
    drop(spool);
    check!(!path.exists());
    Ok(())
}
