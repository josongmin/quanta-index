use super::*;

fn wide_budget() -> Result<WorkBudget, IndexError> {
    Ok(WorkBudget::new(
        WorkLimits {
            reads: 100_000,
            writes: 100_000,
            bytes: 100_000_000,
            mutations: 100_000,
        },
        Instant::now()
            .checked_add(Duration::from_secs(30))
            .ok_or(IndexError::ResourceLimited)?,
    ))
}

#[test]
fn ordered_group_reader_accepts_exact_position_cap_and_refuses_next_before_callback()
-> Result<(), IndexError> {
    let mut work = wide_budget()?;
    let spool = TextSpool::new(&TestScratch, &mut work)?;
    // Private scratch fixture only: this exercises the reader's domain cap,
    // not the producer's writer/admission contract.
    let db = spool.db.scoped(&work)?;
    db.execute_batch(
        "WITH RECURSIVE positions(n) AS (
             SELECT 0 UNION ALL SELECT n+1 FROM positions WHERE n<8192
         ) INSERT INTO tokens(term,pos) SELECT x'0061',n FROM positions;",
    )
    .map_err(|error| sql(&error))?;
    let count: i64 = db
        .query_row("SELECT COUNT(*) FROM tokens", [], |row| row.get(0))
        .map_err(|error| sql(&error))?;
    check_eq!(count, 8193);
    drop(db);

    let mut callbacks = 0usize;
    let rejected = spool.visit_token_groups(8192, &mut work, |_, _, _| {
        callbacks = callbacks
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        Ok(())
    });
    check!(matches!(rejected, Err(IndexError::ResourceLimited)));
    check_eq!(callbacks, 0);

    let db = spool.db.scoped(&work)?;
    check_eq!(
        db.execute("DELETE FROM tokens WHERE term=x'0061' AND pos=8192", [])
            .map_err(|error| sql(&error))?,
        1
    );
    drop(db);
    spool.visit_token_groups(8192, &mut work, |key, positions, _| {
        callbacks = callbacks
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        check_eq!(key.as_slice(), b"\0a");
        check_eq!(positions.as_slice().len(), 8192);
        for (slot, position) in positions.as_slice().iter().enumerate() {
            check_eq!(
                *position,
                u32::try_from(slot).map_err(|_error| IndexError::ResourceLimited)?
            );
        }
        Ok(())
    })?;
    check_eq!(callbacks, 1);
    Ok(())
}
