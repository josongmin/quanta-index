//! One frozen inventory packet needs one global shape probe before map reads.

use super::*;
use quanta_index_core::index::Digest;

const LOWER: &str = "SELECT 1 FROM inventory WHERE root<0 LIMIT 1";
const UPPER: &str = "SELECT 1 FROM inventory WHERE root>=?1 LIMIT 1";

fn probes(spool: &TextSpool, budget: &WorkBudget) -> Result<[i32; 2], IndexError> {
    let db = spool.db.scoped(budget)?;
    let lower = db.prepare_cached(LOWER).map_err(|error| sql(&error))?;
    let first = lower.get_status(rusqlite::StatementStatus::Run);
    let upper = db.prepare_cached(UPPER).map_err(|error| sql(&error))?;
    Ok([first, upper.get_status(rusqlite::StatementStatus::Run)])
}

fn add(spool: &TextSpool, root: u32, budget: &mut WorkBudget) -> Result<(), IndexError> {
    spool.inventory_batch(
        &[(
            InventoryPartition::Ready(root),
            MapEdit {
                key: vec![0, b'a'],
                expected: None,
                next: Some(
                    ObjectRef(Digest(
                        [u8::try_from(root).map_err(|_error| IndexError::ResourceLimited)?; 32],
                    ))
                    .into(),
                ),
            },
        )],
        budget,
    )
}

#[test]
fn frozen_inventory_shape_is_probed_once_across_root_and_global_cursors() -> Result<(), IndexError>
{
    let mut work = budget()?;
    let spool = TextSpool::new(&TestScratch, &mut work)?;
    for root in 0..3 {
        add(&spool, root, &mut work)?;
    }
    let before = probes(&spool, &work)?;
    for root in 0..3 {
        let mut cursor = spool.inventory_root_cursor(root)?;
        check!(cursor.next_edit(&mut work)?.is_some());
        check!(cursor.next_edit(&mut work)?.is_none());
    }
    let mut standalone = spool.inventory_cursor(3)?;
    for _root in 0..3 {
        check!(standalone.next_edit(&mut work)?.is_some());
    }
    check!(standalone.next_edit(&mut work)?.is_none());
    let unshared = probes(&spool, &work)?;
    let expected = before.map(|count| count.saturating_add(4));
    check_eq!(unshared, expected);

    let lease = spool.begin_inventory_read(&mut work)?;
    for root in 0..3 {
        let mut cursor = lease.root_cursor(root)?;
        let row = cursor
            .next_edit(&mut work)?
            .ok_or(IndexError::ResourceLimited)?;
        check_eq!(row.root(), 0);
        check_eq!(row.key(), &[0, b'a']);
        check!(cursor.next_edit(&mut work)?.is_none());
    }
    let mut global = lease.cursor(3)?;
    for root in 0..3 {
        let row = global
            .next_edit(&mut work)?
            .ok_or(IndexError::ResourceLimited)?;
        check_eq!(row.root(), root);
        check_eq!(row.key(), &[0, b'a']);
    }
    check!(global.next_edit(&mut work)?.is_none());
    let after = probes(&spool, &work)?;
    let expected = unshared.map(|count| count.saturating_add(1));
    check_eq!(after, expected);
    drop(global);
    drop(lease);
    spool.reset_inventories(&mut work)?;
    Ok(())
}

#[test]
fn frozen_inventory_rejects_mutation_and_changed_sql_epoch() -> Result<(), IndexError> {
    let mut work = budget()?;
    let spool = TextSpool::new(&TestScratch, &mut work)?;
    add(&spool, 0, &mut work)?;
    let lease = spool.begin_inventory_read(&mut work)?;
    let before = spool.db.scoped(&work)?.total_changes();
    check!(matches!(
        add(&spool, 1, &mut work),
        Err(IndexError::BackendViolation(_))
    ));
    check!(matches!(
        spool.reset_inventories(&mut work),
        Err(IndexError::BackendViolation(_))
    ));
    check_eq!(spool.db.scoped(&work)?.total_changes(), before);
    // Privileged test-only SQL mutation bypasses the public writer guard.
    let changed = spool
        .db
        .scoped(&work)?
        .execute(
            "INSERT INTO inventory(root,k,p,n) VALUES(1,x'0061',NULL,x'01')",
            [],
        )
        .map_err(|error| sql(&error))?;
    check_eq!(changed, 1);
    let mut previous_key = [0; 1024];
    check!(matches!(
        lease.visit_edits(2, &mut previous_key, &mut work, |_, _| Err(
            IndexError::Conflict
        )),
        Err(IndexError::BackendViolation(_))
    ));
    let mut cursor = lease.cursor(2)?;
    check!(matches!(
        cursor.next_edit(&mut work),
        Err(IndexError::BackendViolation(_))
    ));
    Ok(())
}

#[test]
fn borrowed_inventory_keeps_page_reads_and_refuses_foreign_roots() -> Result<(), IndexError> {
    for (roots, bytes) in [(64_u32, 8_320), (65, 8_322)] {
        let mut work = budget()?;
        let spool = TextSpool::new(&TestScratch, &mut work)?;
        for root in 0..roots {
            add(&spool, root, &mut work)?;
        }
        let lease = spool.begin_inventory_read(&mut work)?;
        let roots = usize::try_from(roots).map_err(|_width| IndexError::ResourceLimited)?;
        let mut previous_key = [0; 1024];
        let before = work.usage();
        let mut visited = 0_u32;
        lease.visit_edits(roots, &mut previous_key, &mut work, |row, _| {
            check_eq!(row.root, visited);
            check_eq!(row.key, &[0, b'a']);
            check!(row.expected.is_none());
            check_eq!(row.next.map(<[u8]>::len), Some(33));
            visited = visited.checked_add(1).ok_or(IndexError::ResourceLimited)?;
            Ok(())
        })?;
        let after = work.usage();
        check_eq!(usize::try_from(visited).ok(), Some(roots));
        // Both cases execute two page queries. At 64 rows the second is EOF.
        check_eq!(after.reads.checked_sub(before.reads), Some(65));
        check_eq!(after.bytes.checked_sub(before.bytes), Some(bytes));
        let short = roots.checked_sub(1).ok_or(IndexError::ResourceLimited)?;
        check!(matches!(
            lease.visit_edits(short, &mut previous_key, &mut work, |_, _| Ok(())),
            Err(IndexError::CorruptObject(_))
        ));
    }
    Ok(())
}

#[test]
fn frozen_inventory_refuses_out_of_range_root_before_any_cursor() -> Result<(), IndexError> {
    let mut work = budget()?;
    let spool = TextSpool::new(&TestScratch, &mut work)?;
    add(&spool, 0, &mut work)?;
    let changed = spool
        .db
        .scoped(&work)?
        .execute(
            "INSERT INTO inventory(root,k,p,n) VALUES(-1,x'0061',NULL,x'01')",
            [],
        )
        .map_err(|error| sql(&error))?;
    check_eq!(changed, 1);
    check!(matches!(
        spool.begin_inventory_read(&mut work),
        Err(IndexError::CorruptObject(_))
    ));
    spool.reset_inventories(&mut work)?;
    let _lease = spool.begin_inventory_read(&mut work)?;
    Ok(())
}
