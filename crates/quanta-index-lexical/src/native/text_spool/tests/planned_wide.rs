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
    let total: i64 = spool
        .db
        .scoped(budget)?
        .query_row("SELECT COUNT(*) FROM inventory", [], |row| row.get(0))
        .map_err(|error| sql(&error))?;
    let pending = i64::try_from(spool.inventory_packet.borrow().rows)
        .map_err(|_error| IndexError::ResourceLimited)?;
    Ok((
        total
            .checked_sub(pending)
            .ok_or(IndexError::ResourceLimited)?,
        pending,
    ))
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
        spool
            .db
            .scoped(&work)?
            .execute_batch(
                "CREATE TRIGGER refuse_row_update BEFORE UPDATE ON inventory
             BEGIN SELECT RAISE(ABORT, 'inventory row rewrite'); END;",
            )
            .map_err(|error| sql(&error))?;
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
fn planned_ordinals_above_sql_page_promote_and_stream_in_root_order() -> Result<(), IndexError> {
    let mut work = budget()?;
    let spool = TextSpool::new(&TestScratch, &mut work)?;
    let width = 130_usize;
    let refs = references(width)?;
    for (start, chunk) in refs.chunks(PAGE).enumerate() {
        let edits: Vec<_> = chunk
            .iter()
            .enumerate()
            .map(|(slot, reference)| {
                let ordinal = start
                    .checked_mul(PAGE)
                    .and_then(|start| start.checked_add(slot))
                    .ok_or(IndexError::ResourceLimited)?;
                Ok((
                    InventoryPartition::Planned(
                        u32::try_from(ordinal).map_err(|_error| IndexError::ResourceLimited)?,
                    ),
                    MapEdit {
                        key: b"\0ordinal".to_vec(),
                        expected: None,
                        next: Some((*reference).into()),
                    },
                ))
            })
            .collect::<Result<_, IndexError>>()?;
        spool.inventory_batch(&edits, &mut work)?;
    }
    let receipt = spool.admit_inventory_confirmation(
        &refs,
        u64::try_from(width).map_err(|_error| IndexError::ResourceLimited)?,
        attempt(),
        false,
        &mut work,
    )?;
    receipt.finish(&work)?;
    check_eq!(count_roots(&spool, &work)?, (130, 0));
    let mut cursor = spool.inventory_cursor(width)?;
    for (ordinal, reference) in refs.iter().enumerate() {
        let row = cursor
            .next_edit(&mut work)?
            .ok_or(IndexError::ResourceLimited)?;
        check_eq!(
            row.root,
            u32::try_from(ordinal).map_err(|_error| IndexError::ResourceLimited)?
        );
        check_eq!(row.edit.key.as_slice(), b"\0ordinal");
        check_eq!(row.edit.next, Some((*reference).into()));
    }
    check!(cursor.next_edit(&mut work)?.is_none());
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
    check!(matches!(
        spool.inventory_batch(
            &[(
                InventoryPartition::Planned(0),
                MapEdit {
                    key: b"\0wide-256".to_vec(),
                    expected: None,
                    next: Some((*too_many.get(256).ok_or(IndexError::ResourceLimited)?).into()),
                }
            )],
            &mut work,
        ),
        Err(IndexError::BackendViolation(_))
    ));
    check_eq!(count_roots(&spool, &work)?, (0, 256));
    receipt.finish(&work)?;
    check_eq!(count_roots(&spool, &work)?, (256, 0));
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
            check!(
                spool
                    .inventory_batch(
                        &[(
                            InventoryPartition::Ready(0),
                            MapEdit {
                                key: b"\0wide-064".to_vec(),
                                expected: None,
                                next: Some(reference(300)?.into()),
                            }
                        )],
                        &mut work,
                    )
                    .is_err()
            );
            check_eq!(count_roots(&spool, &work)?, (0, 65));
            check!(matches!(
                spool.admit_inventory_confirmation(&refs, 65, attempt(), true, &mut work),
                Err(IndexError::BackendViolation(_))
            ));
        } else {
            let receipt =
                spool.admit_inventory_confirmation(&refs, 65, attempt(), true, &mut work)?;
            let changed = spool
                .db
                .scoped(&work)?
                .execute(
                    "UPDATE inventory SET n=x'00' WHERE root=?1 AND k=x'00776964652d303634'",
                    [0_i64],
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
fn memo_failure_keeps_packet_unconfirmed() -> Result<(), IndexError> {
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
    check_eq!(count_roots(&spool, &work)?, (0, 65));
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
