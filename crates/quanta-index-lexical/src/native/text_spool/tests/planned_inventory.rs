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
fn partition_codec_refuses_invalid_ordinals_and_corrupt_sql_ranges() -> Result<(), IndexError> {
    let mut work = budget()?;
    let spool = TextSpool::new(&TestScratch, &mut work)?;
    for partition in [
        InventoryPartition::Ready(64),
        InventoryPartition::Planned(64),
    ] {
        let before = work.usage();
        check!(matches!(
            add(&spool, partition, b"\0a", reference(1), &mut work),
            Err(IndexError::InvalidInput(_))
        ));
        check_eq!(work.usage(), before);
    }
    for invalid in [-1_i64, 128_i64] {
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
    add(
        &spool,
        InventoryPartition::Ready(3),
        b"\0a",
        reference(8),
        &mut work,
    )?;
    check!(matches!(
        spool.admit_inventory_confirmation(&[reference(4)], 1, owner, false, &mut work),
        Err(IndexError::Conflict)
    ));
    check!(matches!(
        spool.inventory_cursor(4)?.next_edit(&mut work),
        Err(IndexError::CorruptObject(_))
    ));
    Ok(())
}

#[test]
fn confirmation_sql_failure_leaves_every_fanout_row_planned() -> Result<(), IndexError> {
    let mut work = budget()?;
    let spool = TextSpool::new(&TestScratch, &mut work)?;
    let refs = [reference(10), reference(11)];
    add(
        &spool,
        InventoryPartition::Planned(0),
        b"\0a",
        refs[0],
        &mut work,
    )?;
    add(
        &spool,
        InventoryPartition::Planned(0),
        b"\0b",
        refs[1],
        &mut work,
    )?;
    let receipt = spool.admit_inventory_confirmation(&refs, 2, attempt(), false, &mut work)?;
    spool
        .db
        .scoped(&work)?
        .execute_batch(
            "CREATE TRIGGER refuse_inventory_confirmation BEFORE UPDATE ON inventory
             BEGIN SELECT RAISE(ABORT, 'injected confirmation refusal'); END;",
        )
        .map_err(|error| sql(&error))?;
    check!(matches!(receipt.finish(&work), Err(IndexError::Storage(_))));
    check!(matches!(
        spool.inventory_cursor(1)?.next_edit(&mut work),
        Err(IndexError::CorruptObject(_))
    ));
    let planned: i64 = spool
        .db
        .scoped(&work)?
        .query_row("SELECT COUNT(*) FROM inventory WHERE root>=64", [], |row| {
            row.get(0)
        })
        .map_err(|error| sql(&error))?;
    check_eq!(planned, 2);
    Ok(())
}

#[test]
fn confirmation_denial_and_cancel_do_not_expose_planned_rows() -> Result<(), IndexError> {
    let mut work = budget()?;
    let spool = TextSpool::new(&TestScratch, &mut work)?;
    add(
        &spool,
        InventoryPartition::Planned(0),
        b"\0a",
        reference(12),
        &mut work,
    )?;
    let deadline = Instant::now()
        .checked_add(Duration::from_secs(10))
        .ok_or(IndexError::ResourceLimited)?;
    let mut no_reads = WorkBudget::new(
        WorkLimits {
            reads: 0,
            writes: 100,
            bytes: 1_000_000,
            mutations: 100,
        },
        deadline,
    );
    check!(matches!(
        spool.admit_inventory_confirmation(&[reference(12)], 1, attempt(), false, &mut no_reads),
        Err(IndexError::ResourceLimited)
    ));
    let mut no_writes = WorkBudget::new(
        WorkLimits {
            reads: 100,
            writes: 0,
            bytes: 1_000_000,
            mutations: 100,
        },
        deadline,
    );
    check!(matches!(
        spool.admit_inventory_confirmation(&[reference(12)], 1, attempt(), false, &mut no_writes),
        Err(IndexError::ResourceLimited)
    ));
    check!(matches!(
        spool.inventory_cursor(1)?.next_edit(&mut work),
        Err(IndexError::CorruptObject(_))
    ));
    work.request().cancel_handle().cancel();
    check!(matches!(
        spool.admit_inventory_confirmation(&[reference(12)], 1, attempt(), false, &mut work),
        Err(IndexError::Cancelled)
    ));
    Ok(())
}

#[test]
fn confirmation_charges_typed_null_and_object_rows_before_promotion() -> Result<(), IndexError> {
    let mut literal = [0_u8; 33];
    literal
        .get_mut(1..)
        .ok_or(IndexError::ResourceLimited)?
        .fill(19);
    check_eq!(reference_fixed_bytes(&reference(19))?, literal);
    let encoded = MapValue::Object(reference(19)).encoded()?;
    check_eq!(encoded.as_slice()?, literal.as_slice());

    // Fixed golds: 20,560 for probes and the aggregate check, plus three
    // 108-byte NULL/Object or 140-byte Object/Object row envelopes.
    for (prior, exact) in [(None, 20_884_u64), (Some(reference(18)), 20_980_u64)] {
        for admitted in [false, true] {
            let mut setup = budget()?;
            let spool = TextSpool::new(&TestScratch, &mut setup)?;
            spool.inventory_batch(
                &[(
                    InventoryPartition::Planned(0),
                    MapEdit {
                        key: b"\0a".to_vec(),
                        expected: prior.map(Into::into),
                        next: Some(reference(19).into()),
                    },
                )],
                &mut setup,
            )?;
            let deadline = Instant::now()
                .checked_add(Duration::from_secs(10))
                .ok_or(IndexError::ResourceLimited)?;
            let mut work = WorkBudget::new(
                WorkLimits {
                    reads: 100,
                    writes: 100,
                    bytes: if admitted { exact } else { exact - 1 },
                    mutations: 100,
                },
                deadline,
            );
            let result = spool.admit_inventory_confirmation(
                &[reference(19)],
                1,
                attempt(),
                false,
                &mut work,
            );
            if admitted {
                check_eq!(work.usage().bytes, exact);
                result?.finish(&work)?;
                check!(spool.inventory_cursor(1)?.next_edit(&mut setup)?.is_some());
            } else {
                check!(matches!(result, Err(IndexError::ResourceLimited)));
                check!(matches!(
                    spool.inventory_cursor(1)?.next_edit(&mut setup),
                    Err(IndexError::CorruptObject(_))
                ));
                let planned: i64 = spool
                    .db
                    .scoped(&setup)?
                    .query_row("SELECT COUNT(*) FROM inventory WHERE root>=64", [], |row| {
                        row.get(0)
                    })
                    .map_err(|error| sql(&error))?;
                check_eq!(planned, 1);
            }
        }
    }
    Ok(())
}

#[test]
fn planned_partition_probes_use_the_inventory_primary_key() -> Result<(), IndexError> {
    let mut work = budget()?;
    let spool = TextSpool::new(&TestScratch, &mut work)?;
    for query in [
        "SELECT 1 FROM inventory WHERE root<0 LIMIT 1",
        "SELECT 1 FROM inventory WHERE root>=64 LIMIT 1",
        "SELECT root,k,p,n FROM inventory WHERE root>=64 ORDER BY root,k",
    ] {
        let detail: String = spool
            .db
            .scoped(&work)?
            .query_row(&format!("EXPLAIN QUERY PLAN {query}"), [], |row| row.get(3))
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
fn confirmation_cannot_promote_rows_added_after_its_physical_plan() -> Result<(), IndexError> {
    for replacement in [reference(20), reference(21)] {
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
        add(
            &spool,
            InventoryPartition::Planned(1),
            b"\0b",
            replacement,
            &mut work,
        )?;
        check!(matches!(
            receipt.finish(&work),
            Err(IndexError::BackendViolation(_))
        ));
        let ready: i64 = spool
            .db
            .scoped(&work)?
            .query_row("SELECT COUNT(*) FROM inventory WHERE root<64", [], |row| {
                row.get(0)
            })
            .map_err(|error| sql(&error))?;
        check_eq!(ready, 0);
        check!(spool.inventory_cursor(2)?.next_edit(&mut work).is_err());
    }
    Ok(())
}
