//! Planned inventory rows remain disposable until the physical packet is exact.

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

fn add(
    spool: &TextSpool,
    partition: InventoryPartition,
    key: &[u8],
    next: ObjectRef,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    spool.inventory_batch(
        &[(
            partition,
            MapEdit {
                key: key.to_vec(),
                expected: None,
                next: Some(next.into()),
            },
        )],
        budget,
    )
}

#[test]
fn planned_row_after_first_page_refuses_both_cursors_before_any_ready_prefix()
-> Result<(), IndexError> {
    let mut work = budget()?;
    let spool = TextSpool::new(&TestScratch, &mut work)?;
    for slot in 0..65 {
        let key = format!("\0ready-{slot:03}");
        add(
            &spool,
            InventoryPartition::Ready(0),
            key.as_bytes(),
            reference(1),
            &mut work,
        )?;
    }
    add(
        &spool,
        InventoryPartition::Planned(0),
        b"\0zz-planned",
        reference(2),
        &mut work,
    )?;
    let mut global = spool.inventory_cursor(1)?;
    check!(matches!(
        global.next_edit(&mut work),
        Err(IndexError::CorruptObject(_))
    ));
    check!(global.after.is_none());
    let mut root = spool.inventory_root_cursor(0)?;
    check!(matches!(
        root.next_edit(&mut work),
        Err(IndexError::CorruptObject(_))
    ));
    check!(root.after.is_none());
    let receipt =
        spool.admit_inventory_confirmation(&[reference(2)], 1, attempt(), false, &mut work)?;
    receipt.finish(&work)?;
    let mut global = spool.inventory_cursor(1)?;
    for _slot in 0..66 {
        check!(global.next_edit(&mut work)?.is_some());
    }
    check!(global.next_edit(&mut work)?.is_none());
    Ok(())
}

#[test]
fn existing_inventory_cursors_refuse_new_planned_packet_before_cached_ready_row()
-> Result<(), IndexError> {
    let mut work = budget()?;
    let spool = TextSpool::new(&TestScratch, &mut work)?;
    for key in [b"\0a".as_slice(), b"\0b".as_slice()] {
        add(
            &spool,
            InventoryPartition::Ready(0),
            key,
            reference(1),
            &mut work,
        )?;
    }
    let mut global = spool.inventory_cursor(1)?;
    let mut root = spool.inventory_root_cursor(0)?;
    check!(global.next_edit(&mut work)?.is_some());
    check!(root.next_edit(&mut work)?.is_some());
    add(
        &spool,
        InventoryPartition::Planned(0),
        b"\0z",
        reference(2),
        &mut work,
    )?;
    check!(matches!(
        global.next_edit(&mut work),
        Err(IndexError::CorruptObject(_))
    ));
    check!(matches!(
        root.next_edit(&mut work),
        Err(IndexError::CorruptObject(_))
    ));
    Ok(())
}

#[test]
fn partition_codec_refuses_invalid_ordinals_and_corrupt_sql_ranges() -> Result<(), IndexError> {
    let mut work = budget()?;
    let spool = TextSpool::new(&TestScratch, &mut work)?;
    for partition in [
        InventoryPartition::Ready(PLANNED_ROOT_OFFSET),
        InventoryPartition::Planned(PLANNED_ROOT_OFFSET),
    ] {
        let before = work.usage();
        check!(matches!(
            add(&spool, partition, b"\0a", reference(1), &mut work),
            Err(IndexError::InvalidInput(_))
        ));
        check_eq!(work.usage(), before);
    }
    for invalid in [-1_i64, i64::from(PLANNED_ROOT_END)] {
        let _inserted = spool
            .db
            .scoped(&work)?
            .execute(
                "INSERT INTO inventory(root,k,p,n) VALUES(?1,x'0061',NULL,NULL)",
                [invalid],
            )
            .map_err(|error| sql(&error))?;
        check!(matches!(
            spool.inventory_cursor(1)?.next_edit(&mut work),
            Err(IndexError::CorruptObject(_))
        ));
        spool.reset_inventories(&mut work)?;
    }
    Ok(())
}

#[test]
fn confirmation_preflight_checks_refs_count_fence_and_ready_pk_collision() -> Result<(), IndexError>
{
    let mut work = budget()?;
    let spool = TextSpool::new(&TestScratch, &mut work)?;
    let owner = attempt();
    add(
        &spool,
        InventoryPartition::Planned(3),
        b"\0a",
        reference(4),
        &mut work,
    )?;
    check!(matches!(
        spool.admit_inventory_confirmation(&[reference(5)], 1, owner, false, &mut work),
        Err(IndexError::CorruptObject(_))
    ));
    check!(matches!(
        spool.admit_inventory_confirmation(&[reference(4)], 2, owner, false, &mut work),
        Err(IndexError::BackendViolation(_))
    ));
    check!(matches!(
        spool.admit_inventory_confirmation(
            &[reference(4), reference(9)],
            1,
            owner,
            false,
            &mut work
        ),
        Err(IndexError::BackendViolation(_))
    ));
    check!(matches!(
        spool.admit_inventory_confirmation(
            &[reference(4), reference(4)],
            1,
            owner,
            false,
            &mut work
        ),
        Err(IndexError::BackendViolation(_))
    ));
    check!(matches!(
        spool.admit_inventory_confirmation(
            &[reference(4)],
            1,
            AttemptRef { fence: 2, ..owner },
            false,
            &mut work
        ),
        Err(IndexError::FenceLost)
    ));
    check!(
        add(
            &spool,
            InventoryPartition::Ready(3),
            b"\0a",
            reference(8),
            &mut work,
        )
        .is_err()
    );
    check!(matches!(
        spool.admit_inventory_confirmation(&[reference(4)], 1, owner, false, &mut work),
        Err(IndexError::BackendViolation(_))
    ));
    check!(matches!(
        spool.inventory_cursor(4)?.next_edit(&mut work),
        Err(IndexError::BackendViolation(_))
    ));
    check!(matches!(
        spool.reset_inventories(&mut work),
        Err(IndexError::BackendViolation(_))
    ));
    check!(matches!(
        add(
            &spool,
            InventoryPartition::Planned(3),
            b"\0b",
            reference(9),
            &mut work
        ),
        Err(IndexError::BackendViolation(_))
    ));
    let fresh = TextSpool::new(&TestScratch, &mut work)?;
    add(
        &fresh,
        InventoryPartition::Planned(3),
        b"\0a",
        reference(4),
        &mut work,
    )?;
    fresh
        .admit_inventory_confirmation(&[reference(4)], 1, owner, false, &mut work)?
        .finish(&work)?;
    Ok(())
}

#[test]
fn out_of_band_sql_after_admission_poison_refuses_packet() -> Result<(), IndexError> {
    let mut work = budget()?;
    let spool = TextSpool::new(&TestScratch, &mut work)?;
    let refs = [reference(10), reference(11)];
    for (key, reference) in [(b"\0a".as_slice(), refs[0]), (b"\0b".as_slice(), refs[1])] {
        add(
            &spool,
            InventoryPartition::Planned(0),
            key,
            reference,
            &mut work,
        )?;
    }
    let receipt = spool.admit_inventory_confirmation(&refs, 2, attempt(), false, &mut work)?;
    // The private connection is inaccessible to product callers. This direct
    // mutation exercises the independent SQL total-changes fence.
    let inserted = spool
        .db
        .scoped(&work)?
        .execute(
            "INSERT INTO inventory(root,k,p,n) VALUES(0,x'007a',NULL,?1)",
            [reference_bytes(&refs[0])?],
        )
        .map_err(|error| sql(&error))?;
    check_eq!(inserted, 1);
    check!(matches!(
        receipt.finish(&work),
        Err(IndexError::BackendViolation(_))
    ));
    check!(matches!(
        spool.inventory_cursor(1)?.next_edit(&mut work),
        Err(IndexError::BackendViolation(_))
    ));
    check_eq!(spool.inventory_packet.borrow().rows, 2);
    Ok(())
}

#[test]
fn confirmation_cancel_does_not_expose_planned_rows() -> Result<(), IndexError> {
    let mut work = budget()?;
    let spool = TextSpool::new(&TestScratch, &mut work)?;
    add(
        &spool,
        InventoryPartition::Planned(0),
        b"\0a",
        reference(12),
        &mut work,
    )?;
    check!(matches!(
        spool.inventory_cursor(1)?.next_edit(&mut work),
        Err(IndexError::CorruptObject(_))
    ));
    let receipt =
        spool.admit_inventory_confirmation(&[reference(12)], 1, attempt(), false, &mut work)?;
    work.request().cancel_handle().cancel();
    check!(matches!(receipt.finish(&work), Err(IndexError::Cancelled)));
    let mut fresh = budget()?;
    check!(matches!(
        spool.reset_inventories(&mut fresh),
        Err(IndexError::BackendViolation(_))
    ));
    Ok(())
}

#[test]
fn admitted_packet_does_not_recharge_inserted_term_rows() -> Result<(), IndexError> {
    let mut setup = budget()?;
    let spool = TextSpool::new(&TestScratch, &mut setup)?;
    add(
        &spool,
        InventoryPartition::Planned(0),
        b"\0a",
        reference(19),
        &mut setup,
    )?;
    let deadline = Instant::now()
        .checked_add(Duration::from_secs(10))
        .ok_or(IndexError::ResourceLimited)?;
    let mut work = WorkBudget::new(
        WorkLimits {
            reads: 0,
            writes: 0,
            bytes: 0,
            mutations: 0,
        },
        deadline,
    );
    let before = work.usage();
    let receipt =
        spool.admit_inventory_confirmation(&[reference(19)], 1, attempt(), false, &mut work)?;
    check_eq!(work.usage(), before);
    receipt.finish(&work)?;
    check!(spool.inventory_cursor(1)?.next_edit(&mut setup)?.is_some());
    Ok(())
}

#[test]
fn planned_partition_probes_use_the_inventory_primary_key() -> Result<(), IndexError> {
    let mut work = budget()?;
    let spool = TextSpool::new(&TestScratch, &mut work)?;
    for (query, params) in [
        ("SELECT 1 FROM inventory WHERE root<0 LIMIT 1", None),
        (
            "SELECT 1 FROM inventory WHERE root>=?1 LIMIT 1",
            Some(i64::from(PLANNED_ROOT_OFFSET)),
        ),
    ] {
        let plan = format!("EXPLAIN QUERY PLAN {query}");
        let db = spool.db.scoped(&work)?;
        let detail: String = match params {
            Some(root) => db.query_row(&plan, [root], |row| row.get(3)),
            None => db.query_row(&plan, [], |row| row.get(3)),
        }
        .map_err(|error| sql(&error))?;
        check!(
            detail.contains("SEARCH inventory USING PRIMARY KEY"),
            "{detail}"
        );
    }
    add(
        &spool,
        InventoryPartition::Planned(0),
        b"\0a",
        reference(13),
        &mut work,
    )?;
    let receipt =
        spool.admit_inventory_confirmation(&[reference(13)], 1, attempt(), false, &mut work)?;
    receipt.finish(&work)?;
    Ok(())
}

#[test]
fn writer_refuses_late_row_during_physical_receipt() -> Result<(), IndexError> {
    let mut work = budget()?;
    let spool = TextSpool::new(&TestScratch, &mut work)?;
    add(
        &spool,
        InventoryPartition::Planned(0),
        b"\0a",
        reference(20),
        &mut work,
    )?;
    let receipt =
        spool.admit_inventory_confirmation(&[reference(20)], 1, attempt(), false, &mut work)?;
    check!(matches!(
        add(
            &spool,
            InventoryPartition::Planned(1),
            b"\0b",
            reference(21),
            &mut work
        ),
        Err(IndexError::BackendViolation(_))
    ));
    receipt.finish(&work)?;
    let mut cursor = spool.inventory_cursor(2)?;
    check!(cursor.next_edit(&mut work)?.is_some());
    check!(cursor.next_edit(&mut work)?.is_none());
    Ok(())
}

#[test]
fn reset_reuses_stable_key_only_after_completed_packet() -> Result<(), IndexError> {
    let mut work = budget()?;
    let spool = TextSpool::new(&TestScratch, &mut work)?;
    for reference in [reference(41), reference(42)] {
        add(
            &spool,
            InventoryPartition::Planned(0),
            b"\0same-key",
            reference,
            &mut work,
        )?;
        check!(matches!(
            spool.reset_inventories(&mut work),
            Err(IndexError::BackendViolation(_))
        ));
        let receipt =
            spool.admit_inventory_confirmation(&[reference], 1, attempt(), false, &mut work)?;
        receipt.finish(&work)?;
        let row = spool
            .inventory_cursor(1)?
            .next_edit(&mut work)?
            .ok_or(IndexError::ResourceLimited)?;
        check_eq!(row.edit.next, Some(reference.into()));
        spool.reset_inventories(&mut work)?;
    }
    Ok(())
}
