//! A complete physical install envelope may span four 64-row SQL packets.

use super::*;
use quanta_index_core::index::{
    AttemptId, CollectionId, Digest, OperationKey, ProducerId, SlotId, StreamId, TenantId,
    WorkspaceKey, WorktreeId,
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

fn reference(index: usize) -> Result<ObjectRef, IndexError> {
    let ordinal = u16::try_from(index.checked_add(1).ok_or(IndexError::ResourceLimited)?)
        .map_err(|_error| IndexError::ResourceLimited)?;
    let mut bytes = [0_u8; 32];
    bytes
        .get_mut(..2)
        .ok_or(IndexError::ResourceLimited)?
        .copy_from_slice(&ordinal.to_be_bytes());
    Ok(ObjectRef(Digest(bytes)))
}

fn references(count: usize) -> Result<Vec<ObjectRef>, IndexError> {
    (0..count).map(reference).collect()
}

fn add_planned(
    spool: &TextSpool,
    references: &[ObjectRef],
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    for (packet, chunk) in references.chunks(PAGE).enumerate() {
        let start = packet
            .checked_mul(PAGE)
            .ok_or(IndexError::ResourceLimited)?;
        let edits: Vec<_> = chunk
            .iter()
            .enumerate()
            .map(|(offset, reference)| {
                let index = start
                    .checked_add(offset)
                    .ok_or(IndexError::ResourceLimited)?;
                Ok((
                    InventoryPartition::Planned(0),
                    MapEdit {
                        key: format!("\0wide-{index:03}").into_bytes(),
                        expected: None,
                        next: Some((*reference).into()),
                    },
                ))
            })
            .collect::<Result<_, IndexError>>()?;
        spool.inventory_batch(&edits, budget)?;
    }
    Ok(())
}

fn count_roots(spool: &TextSpool, budget: &WorkBudget) -> Result<(i64, i64), IndexError> {
    spool
        .db
        .scoped(budget)?
        .query_row(
            "SELECT COUNT(CASE WHEN root<64 THEN 1 END),\
                    COUNT(CASE WHEN root>=64 THEN 1 END) FROM inventory",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|error| sql(&error))
}

#[test]
fn complete_envelopes_64_65_and_256_promote_every_row_and_memo() -> Result<(), IndexError> {
    for width in [64_usize, 65, 256] {
        let mut work = budget()?;
        let spool = TextSpool::new(&TestScratch, &mut work)?;
        let refs = references(width)?;
        add_planned(&spool, &refs, &mut work)?;
        let receipt = spool.admit_inventory_confirmation(
            &refs,
            u64::try_from(width).map_err(|_error| IndexError::ResourceLimited)?,
            attempt(),
            true,
            &mut work,
        )?;
        receipt.finish(&work)?;
        check_eq!(
            count_roots(&spool, &work)?,
            (
                i64::try_from(width).map_err(|_error| IndexError::ResourceLimited)?,
                0
            )
        );
        for reference in refs {
            check_eq!(
                spool.confirmed_occurrence_count(reference, attempt(), &mut work)?,
                ConfirmedOccurrenceCount::Unvalidated
            );
        }
    }
    Ok(())
}

#[test]
fn wide_envelope_refuses_foreign_and_late_planned_rows() -> Result<(), IndexError> {
    let mut work = budget()?;
    let spool = TextSpool::new(&TestScratch, &mut work)?;
    let refs = references(256)?;
    add_planned(&spool, &refs, &mut work)?;
    let too_many = references(257)?;
    check!(matches!(
        spool.admit_inventory_confirmation(&too_many, 256, attempt(), false, &mut work),
        Err(IndexError::InvalidInput(_))
    ));
    let receipt = spool.admit_inventory_confirmation(&refs, 256, attempt(), false, &mut work)?;
    spool.inventory_batch(
        &[(
            InventoryPartition::Planned(0),
            MapEdit {
                key: b"\0wide-256".to_vec(),
                expected: None,
                next: Some((*too_many.get(256).ok_or(IndexError::ResourceLimited)?).into()),
            },
        )],
        &mut work,
    )?;
    check!(matches!(
        receipt.finish(&work),
        Err(IndexError::BackendViolation(_))
    ));
    check_eq!(count_roots(&spool, &work)?, (0, 257));
    Ok(())
}

#[test]
fn preflight_refuses_unrelated_planned_row_beyond_first_sql_packet() -> Result<(), IndexError> {
    let mut work = budget()?;
    let spool = TextSpool::new(&TestScratch, &mut work)?;
    let refs = references(65)?;
    add_planned(&spool, &refs, &mut work)?;
    spool.inventory_batch(
        &[(
            InventoryPartition::Planned(0),
            MapEdit {
                key: b"\0wide-065".to_vec(),
                expected: None,
                next: Some(reference(300)?.into()),
            },
        )],
        &mut work,
    )?;
    check!(matches!(
        spool.admit_inventory_confirmation(&refs, 66, attempt(), true, &mut work),
        Err(IndexError::CorruptObject(_))
    ));
    check_eq!(count_roots(&spool, &work)?, (0, 66));
    Ok(())
}

#[test]
fn wide_envelope_checks_last_ref_and_ready_pk_collision_before_promotion() -> Result<(), IndexError>
{
    for collision in [false, true] {
        let mut work = budget()?;
        let spool = TextSpool::new(&TestScratch, &mut work)?;
        let refs = references(65)?;
        add_planned(&spool, &refs, &mut work)?;
        if collision {
            spool.inventory_batch(
                &[(
                    InventoryPartition::Ready(0),
                    MapEdit {
                        key: b"\0wide-064".to_vec(),
                        expected: None,
                        next: Some(reference(300)?.into()),
                    },
                )],
                &mut work,
            )?;
            check!(matches!(
                spool.admit_inventory_confirmation(&refs, 65, attempt(), true, &mut work),
                Err(IndexError::Conflict)
            ));
            check_eq!(count_roots(&spool, &work)?, (1, 65));
        } else {
            let receipt =
                spool.admit_inventory_confirmation(&refs, 65, attempt(), true, &mut work)?;
            let changed = spool
                .db
                .scoped(&work)?
                .execute(
                    "UPDATE inventory SET n=x'00' WHERE root=64 AND k=x'00776964652d303634'",
                    [],
                )
                .map_err(|error| sql(&error))?;
            check_eq!(changed, 1);
            check!(matches!(
                receipt.finish(&work),
                Err(IndexError::BackendViolation(_))
            ));
            check_eq!(count_roots(&spool, &work)?, (0, 65));
        }
    }
    Ok(())
}

#[test]
fn memo_failure_propagates_after_complete_pk_promotion() -> Result<(), IndexError> {
    let mut work = budget()?;
    let spool = TextSpool::new(&TestScratch, &mut work)?;
    let refs = references(65)?;
    add_planned(&spool, &refs, &mut work)?;
    let receipt = spool.admit_inventory_confirmation(&refs, 65, attempt(), true, &mut work)?;
    spool
        .db
        .scoped(&work)?
        .execute_batch(
            "CREATE TRIGGER refuse_wide_memo BEFORE INSERT ON confirmed_occurrence \
             BEGIN SELECT RAISE(ABORT, 'injected confirmed memo refusal'); END;",
        )
        .map_err(|error| sql(&error))?;
    check!(matches!(receipt.finish(&work), Err(IndexError::Storage(_))));
    check_eq!(count_roots(&spool, &work)?, (65, 0));
    let memo_count: i64 = spool
        .db
        .scoped(&work)?
        .query_row("SELECT COUNT(*) FROM confirmed_occurrence", [], |row| {
            row.get(0)
        })
        .map_err(|error| sql(&error))?;
    check_eq!(memo_count, 0);
    Ok(())
}
