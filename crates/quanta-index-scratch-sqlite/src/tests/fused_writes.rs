use super::*;
use quanta_index_core::index::{
    MapEdit, MemoryLease, MemoryLifetime, MemoryScope, MemoryScopePort,
};
use rusqlite::{StatementStatus, types::ValueRef};

const PREFIX: &str = "INSERT INTO edit(root,key,expected,next) VALUES ";

fn work(bytes: u64, writes: u64) -> Result<WorkBudget, IndexError> {
    Ok(WorkBudget::new(
        WorkLimits {
            reads: 0,
            writes,
            bytes,
            mutations: 0,
        },
        Instant::now()
            .checked_add(Duration::from_secs(10))
            .ok_or(IndexError::ResourceLimited)?,
    ))
}

fn fixture() -> Result<(TestAllocation, Connection), IndexError> {
    let allocation = TestAllocation::new(256 * 1024, 4 * 1024 * 1024, false)?;
    let db = allocation.open()?;
    configure(&db, &allocation)?;
    db.execute_batch("CREATE TABLE edit(root BLOB NOT NULL,key BLOB NOT NULL,expected BLOB,next BLOB,PRIMARY KEY(root,key)) WITHOUT ROWID;")
        .map_err(|error| sql(&error))?;
    Ok((allocation, db))
}

#[test]
fn sixty_four_rows_use_one_sql_run_and_keep_original_row_and_query_funding()
-> Result<(), Box<dyn std::error::Error>> {
    let (allocation, db) = fixture()?;
    let inline = MapValue::inline(&[9; 32], [ObjectRef(Digest([5; 32])); 5])?;
    let mut edits = Vec::new();
    for index in 0_u16..64 {
        edits.push((
            u32::from(index & 1),
            MapEdit {
                key: index.to_be_bytes().to_vec(),
                expected: if index & 1 == 0 {
                    None
                } else {
                    Some(ObjectRef(Digest([7; 32])).into())
                },
                next: if index & 2 == 0 { Some(inline) } else { None },
            },
        ));
    }
    // Independent writer budget:64 rows must fit64KiB, without changing the
    // logical write limit. Per-row4096-byte executions cannot satisfy it.
    let mut budget = work(64 * 1024, 64)?;
    write_map_edit_batch(
        &db,
        &allocation,
        PREFIX,
        4096,
        &[1024; 64],
        |slot| {
            let (root, edit) = edits.get(slot).ok_or(IndexError::ResourceLimited)?;
            Ok((ValueRef::Integer(i64::from(*root)), edit, 0))
        },
        &mut budget,
    )?;
    check_eq!(budget.usage().writes, 64);
    let query = format!(
        "{PREFIX}{}",
        std::iter::repeat_n("(?,?,?,?)", 64)
            .collect::<Vec<_>>()
            .join(",")
    );
    let prepared = db.prepare_cached(&query).map_err(|error| sql(&error))?;
    check_eq!(prepared.get_status(StatementStatus::Run), 1);
    drop(prepared);
    let mut statement = db
        .prepare("SELECT key,expected,next,root FROM edit ORDER BY key")
        .map_err(|error| sql(&error))?;
    let mut rows = statement.query([]).map_err(|error| sql(&error))?;
    let mut seen = 0_usize;
    while let Some(row) = rows.next().map_err(|error| sql(&error))? {
        let (root, expected) = edits.get(seen).ok_or(IndexError::ResourceLimited)?;
        check_eq!(
            row.get::<_, i64>(3).map_err(|error| sql(&error))?,
            i64::from(*root)
        );
        let actual = read_map_edit(row, 4096, *root, &budget)?;
        check_eq!(actual.edit.key, expected.key);
        check_eq!(actual.edit.expected, expected.expected);
        check_eq!(actual.edit.next, expected.next);
        seen = seen.checked_add(1).ok_or(IndexError::ResourceLimited)?;
    }
    check_eq!(seen, 64);
    Ok(())
}

struct DenyMemory;
impl MemoryScopePort for DenyMemory {
    fn reserve(&self, _bytes: u64, _lifetime: MemoryLifetime) -> Result<MemoryLease, IndexError> {
        Err(IndexError::ResourceLimited)
    }
}

#[test]
fn original_fee_write_limit_and_sql_backing_refuse_before_any_mutation()
-> Result<(), Box<dyn std::error::Error>> {
    for (bytes, writes, deny_memory) in [
        (4096, 1, false),
        (64 * 1024, 0, false),
        (64 * 1024, 1, true),
    ] {
        let (allocation, db) = fixture()?;
        let edit = MapEdit {
            key: vec![0, 1],
            expected: None,
            next: Some(ObjectRef(Digest([4; 32])).into()),
        };
        let mut budget = work(bytes, writes)?;
        if deny_memory {
            budget.bind_memory_scope(Some(MemoryScope::new(Arc::new(DenyMemory))));
        }
        let result = write_map_edit_batch(
            &db,
            &allocation,
            PREFIX,
            4096,
            &[1024],
            |_slot| Ok((ValueRef::Integer(3), &edit, 0)),
            &mut budget,
        );
        check!(matches!(result, Err(IndexError::ResourceLimited)));
        let count: i64 = db
            .query_row("SELECT count(*) FROM edit", [], |row| row.get(0))
            .map_err(|error| sql(&error))?;
        check_eq!(count, 0);
    }
    Ok(())
}

#[test]
fn blob_partition_is_bound_and_last_bind_cancellation_never_executes_sql()
-> Result<(), Box<dyn std::error::Error>> {
    let (allocation, db) = fixture()?;
    let edit = MapEdit {
        key: vec![0, 1],
        expected: None,
        next: Some(ObjectRef(Digest([4; 32])).into()),
    };
    let mut budget = work(64 * 1024, 2)?;
    write_map_edit_batch(
        &db,
        &allocation,
        PREFIX,
        4096,
        &[1024],
        |_slot| Ok((ValueRef::Blob(b"\0term"), &edit, 23)),
        &mut budget,
    )?;
    let actual: Vec<u8> = db
        .query_row("SELECT root FROM edit", [], |row| row.get(0))
        .map_err(|error| sql(&error))?;
    check_eq!(actual, b"\0term".to_vec());
    let cancel = budget.request().cancel_handle();
    let calls = std::cell::Cell::new(0_u8);
    let result = write_map_edit_batch(
        &db,
        &allocation,
        PREFIX,
        4096,
        &[1024],
        |_slot| {
            let count = calls
                .get()
                .checked_add(1)
                .ok_or(IndexError::ResourceLimited)?;
            calls.set(count);
            if count == 2 {
                cancel.cancel();
            }
            Ok((ValueRef::Blob(b"not-inserted"), &edit, 0))
        },
        &mut budget,
    );
    check!(matches!(result, Err(IndexError::Cancelled)));
    let count: i64 = db
        .query_row("SELECT count(*) FROM edit", [], |row| row.get(0))
        .map_err(|error| sql(&error))?;
    check_eq!(count, 1);
    Ok(())
}
