//! Cold directory descriptors share the planned inventory receipt and cursor.

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

fn reference(byte: u8) -> ObjectRef {
    ObjectRef(Digest([byte; 32]))
}

fn descriptor(reference: ObjectRef) -> Result<MapValue, IndexError> {
    MapValue::inline(&[1, 0, 2], [reference])
}

fn row(key: &[u8], expected: Option<MapValue>, next: Option<MapValue>) -> MapEdit {
    MapEdit {
        key: key.to_vec(),
        expected,
        next,
    }
}

#[test]
fn planned_directory_descriptor_binds_child_receipt_and_ready_row_reads() -> Result<(), IndexError>
{
    let mut work = budget()?;
    let spool = TextSpool::new(&TestScratch, &mut work)?;
    let ready = descriptor(reference(3))?;
    let planned = descriptor(reference(4))?;
    spool.inventory_batch(
        &[(InventoryPartition::Ready(0), row(b"\0a", None, Some(ready)))],
        &mut work,
    )?;
    spool.inventory_batch(
        &[(
            InventoryPartition::Planned(0),
            row(b"\0z", None, Some(planned)),
        )],
        &mut work,
    )?;
    check!(matches!(
        spool.inventory_root_cursor(0)?.next_edit(&mut work),
        Err(IndexError::CorruptObject(_))
    ));
    check!(matches!(
        spool.admit_inventory_confirmation(&[reference(5)], 1, attempt(), false, &mut work),
        Err(IndexError::CorruptObject(_))
    ));
    let receipt =
        spool.admit_inventory_confirmation(&[reference(4)], 1, attempt(), false, &mut work)?;
    receipt.finish(&work)?;
    let mut cursor = spool.inventory_root_cursor(0)?;
    let first = cursor
        .next_edit(&mut work)?
        .ok_or(IndexError::ResourceLimited)?;
    check_eq!(first.key(), b"\0a".to_vec());
    check_eq!(first.next()?, Some(ready));
    let second = cursor
        .next_edit(&mut work)?
        .ok_or(IndexError::ResourceLimited)?;
    check_eq!(second.key(), b"\0z".to_vec());
    check_eq!(second.next()?, Some(planned));
    check!(cursor.next_edit(&mut work)?.is_none());
    Ok(())
}

#[test]
fn inventory_descriptor_preflight_rejects_malformed_values_before_sql() -> Result<(), IndexError> {
    let mut work = budget()?;
    let spool = TextSpool::new(&TestScratch, &mut work)?;
    let malformed = [
        MapValue::inline(&[1, 0, 1], [reference(1)])?,
        MapValue::inline(&[3, 0, 2], [reference(1)])?,
        MapValue::inline(&[1, 0, 2], [])?,
        MapValue::inline(&[1, 0, 2], [reference(1), reference(2)])?,
        MapValue::inline(&[1, 0, 2, 0], [reference(1)])?,
    ];
    for partition in [InventoryPartition::Ready(0), InventoryPartition::Planned(0)] {
        for value in malformed {
            let before = spool.db.change_count()?;
            check!(
                spool
                    .inventory_batch(&[(partition, row(b"\0a", None, Some(value)))], &mut work)
                    .is_err()
            );
            check_eq!(spool.db.change_count()?, before);
        }
    }
    for partition in [InventoryPartition::Ready(0), InventoryPartition::Planned(0)] {
        let before = spool.db.change_count()?;
        check!(
            spool
                .inventory_batch(
                    &[(
                        partition,
                        row(b"\0a", Some(descriptor(reference(1))?), None)
                    )],
                    &mut work,
                )
                .is_err()
        );
        check_eq!(spool.db.change_count()?, before);
    }
    let before = spool.db.change_count()?;
    check!(
        spool
            .inventory_batch(
                &[(InventoryPartition::Planned(0), row(b"\0a", None, None))],
                &mut work,
            )
            .is_err()
    );
    check_eq!(spool.db.change_count()?, before);
    spool.inventory_batch(
        &[(
            InventoryPartition::Ready(0),
            row(b"\0a", None, Some(descriptor(reference(1))?)),
        )],
        &mut work,
    )?;
    Ok(())
}
