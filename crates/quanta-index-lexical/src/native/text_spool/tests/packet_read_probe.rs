use super::*;

// Observe actual point-query executions, separately from WorkBudget reads.
// SQLite pager cache hits still execute the query and materialize its BLOB.
// Set capacity on the fixture so unrelated statements cannot evict its counter.
impl TextSpool {
    pub(in crate::native) fn posting_packet_read_probe(
        &self,
        budget: &WorkBudget,
    ) -> Result<(i32, u64), IndexError> {
        let db = self.db.scoped(budget)?;
        db.set_prepared_statement_cache_capacity(128);
        let (packets, bytes): (i64, i64) = db
            .query_row(
                "SELECT count(*),coalesce(sum(length(payload)),0) FROM posting_packet",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(|error| sql(&error))?;
        check_eq!(packets, 1);
        let statement = db
            .prepare_cached(
                "SELECT first_term,first_owner,last_term,last_owner,payload FROM posting_packet
             WHERE phase=?1 AND run=?2 AND page=?3",
            )
            .map_err(|error| sql(&error))?;
        let bytes = u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)?;
        Ok((statement.get_status(rusqlite::StatementStatus::Run), bytes))
    }
}

#[test]
fn packed_reposition_preserves_old_position_on_denial_and_rejects_changed_epoch()
-> Result<(), IndexError> {
    let mut work = budget()?;
    let spool = TextSpool::new(&TestScratch, &mut work)?;
    spool.posting_batch(
        &[
            edit(b"\0a", 1, None, Some(object(31))),
            edit(b"\0b", 2, None, Some(object(32))),
            edit(b"\x01a", 3, None, Some(object(33))),
        ],
        &mut work,
    )?;
    let _terms = spool.posting_term_page(None, &mut work)?;
    let mut reader = PackedRangeCursor::new(&spool, b"\0a", &mut work)?;
    let first = reader.next(&mut work)?.ok_or(IndexError::Conflict)?;
    check_eq!(first.term.as_slice(), b"\0a");
    let mut denied = WorkBudget::new(
        WorkLimits {
            reads: 0,
            writes: 0,
            bytes: 8192,
            mutations: 0,
        },
        work.request().deadline(),
    );
    check_eq!(
        reader.reposition(b"\x01a", &mut denied),
        Err(IndexError::ResourceLimited)
    );
    let second = reader.next(&mut work)?.ok_or(IndexError::Conflict)?;
    check_eq!(second.term.as_slice(), b"\0b");
    let (before, _) = spool.posting_packet_read_probe(&work)?;
    reader.reposition(b"\x01a", &mut work)?;
    let switched = reader.next(&mut work)?.ok_or(IndexError::Conflict)?;
    check_eq!(switched.term.as_slice(), b"\x01a");
    check_eq!(switched.next, Some(object(33)));
    check_eq!(spool.posting_packet_read_probe(&work)?.0, before);

    let db = spool.db.scoped(&work)?;
    let updated = db
        .execute("UPDATE posting_packet SET last_term=x'017a'", [])
        .map_err(|error| sql(&error))?;
    check_eq!(updated, 1);
    drop(db);
    check!(matches!(
        reader.reposition(b"\x01a", &mut work),
        Err(IndexError::BackendViolation(_))
    ));
    check!(matches!(
        reader.next(&mut work),
        Err(IndexError::BackendViolation(_))
    ));
    // A new reader must authenticate the current SQL fences against the
    // payload, even though the existing reader already checked that packet.
    let mut fresh = PackedRangeCursor::new(&spool, b"\x01a", &mut work)?;
    check!(matches!(
        fresh.next(&mut work),
        Err(IndexError::CorruptObject(_))
    ));
    Ok(())
}

#[test]
fn packed_append_flush_refusal_retries_only_before_batch_effects() -> Result<(), IndexError> {
    let rows: Vec<_> = (0..127)
        .map(|index| {
            let term = format!("\0{index:03}{}", "x".repeat(1020));
            edit(term.as_bytes(), 1, None, Some(object(31)))
        })
        .collect();
    for buffered in [126, 125] {
        let mut work = budget()?;
        let spool = TextSpool::new(&TestScratch, &mut work)?;
        spool.posting_batch(&rows[..buffered], &mut work)?;
        let mut denied = WorkBudget::new(
            WorkLimits {
                reads: 1000,
                writes: u64::try_from(rows.len() - buffered)
                    .map_err(|_error| IndexError::ResourceLimited)?,
                bytes: 10_000_000,
                mutations: 1000,
            },
            work.request().deadline(),
        );
        let epoch = spool.db.change_count()?;
        check_eq!(
            spool.posting_batch(&rows[buffered..], &mut denied),
            Err(IndexError::ResourceLimited)
        );
        check_eq!(spool.db.change_count()?, epoch);
        if buffered == 125 {
            // The first edit of this batch is already buffered. A retry
            // would duplicate it, even though SQLite has not changed.
            check!(matches!(
                spool.posting_batch(&rows[buffered..], &mut work),
                Err(IndexError::BackendViolation(_))
            ));
            continue;
        }
        spool.posting_batch(&rows[buffered..], &mut work)?;
        let mut cursor = PackedRangeCursor::new(&spool, &rows[0].0, &mut work)?;
        for (term, expected) in &rows {
            let actual = cursor.next(&mut work)?.ok_or(IndexError::Conflict)?;
            check_eq!(&actual.term, term);
            check_eq!(actual.owner.as_slice(), expected.key.as_slice());
            check_eq!(actual.next, expected.next);
        }
        check!(cursor.next(&mut work)?.is_none());
    }
    Ok(())
}
