use super::*;
use quanta_index_core::index::{
    MapValue, MemoryLease, MemoryLeaseOwner, MemoryLifetime, MemoryScope, MemoryScopePort,
};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

fn scan(reads: u64) -> Result<WorkBudget, IndexError> {
    Ok(WorkBudget::new(
        WorkLimits {
            reads,
            writes: 0,
            bytes: 2_000_000,
            mutations: 0,
        },
        Instant::now()
            .checked_add(Duration::from_secs(10))
            .ok_or(IndexError::ResourceLimited)?,
    ))
}

fn owner(index: u16) -> Vec<u8> {
    let mut key = vec![0; 30];
    key.extend_from_slice(&index.to_be_bytes());
    key
}

fn range_runs(spool: &TextSpool, budget: &WorkBudget) -> Result<i64, IndexError> {
    let db = spool.db.scoped(budget)?;
    [POSTING_RANGE_FIRST_SQL, POSTING_RANGE_NEXT_SQL]
        .into_iter()
        .try_fold(0_i64, |total, query| {
            let statement = db.prepare_cached(query).map_err(|error| sql(&error))?;
            total
                .checked_add(i64::from(
                    statement.get_status(rusqlite::StatementStatus::Run),
                ))
                .ok_or(IndexError::ResourceLimited)
        })
}

#[test]
fn posting_range_129_terms_preserves_raw_pk_tuples_and_ordinal_gold() -> Result<(), IndexError> {
    let mut setup = budget()?;
    let spool = TextSpool::new(&TestScratch, &mut setup)?;
    let first = ObjectRef(quanta_index_core::index::Digest([31; 32]));
    let second = ObjectRef(quanta_index_core::index::Digest([32; 32]));
    // Fixed wire gold is independent of MapValue::encoded and the cursor.
    let mut first_wire = vec![0];
    first_wire.extend_from_slice(&[31; 32]);
    let mut second_wire = vec![0];
    second_wire.extend_from_slice(&[32; 32]);
    let mut raw_gold = Vec::new();
    for index in 0_u16..129 {
        let term = format!("\0term-{index:03}").into_bytes();
        let key = owner(index);
        let reference = if index.is_multiple_of(2) {
            first
        } else {
            second
        };
        spool.posting_batch(
            &[(
                term.clone(),
                MapEdit {
                    key: key.clone(),
                    expected: None,
                    next: Some(MapValue::Object(reference)),
                },
            )],
            &mut setup,
        )?;
        raw_gold.push((
            term,
            key,
            None::<Vec<u8>>,
            Some(if index.is_multiple_of(2) {
                first_wire.clone()
            } else {
                second_wire.clone()
            }),
        ));
    }
    let db = spool.db.scoped(&setup)?;
    let mut raw_statement = db
        .prepare_cached("SELECT term,k,p,n FROM posting ORDER BY term,k")
        .map_err(|error| sql(&error))?;
    let mut raw_rows = raw_statement.query([]).map_err(|error| sql(&error))?;
    let mut raw_observed = Vec::new();
    while let Some(row) = raw_rows.next().map_err(|error| sql(&error))? {
        raw_observed.push((
            row.get::<_, Vec<u8>>(0).map_err(|error| sql(&error))?,
            row.get::<_, Vec<u8>>(1).map_err(|error| sql(&error))?,
            row.get::<_, Option<Vec<u8>>>(2)
                .map_err(|error| sql(&error))?,
            row.get::<_, Option<Vec<u8>>>(3)
                .map_err(|error| sql(&error))?,
        ));
    }
    check_eq!(raw_observed, raw_gold);
    drop(raw_rows);
    drop(raw_statement);
    drop(db);

    // TermPage is the established 64-term admission boundary. A full page
    // requires its EOF seek; 64 + 64 + 1 rows therefore need 2 + 2 + 1
    // range statements, while the previous one-term cursor needed 129.
    let mut work = scan(400)?;
    let before = range_runs(&spool, &setup)?;
    let mut after: Option<Vec<u8>> = None;
    let mut observed = 0_u16;
    let mut page_sizes = Vec::new();
    loop {
        let terms = spool.posting_term_page(after.as_deref(), &mut work)?;
        if terms.is_empty() {
            break;
        }
        page_sizes.push(terms.len());
        after = terms.last().map(|key| key.as_slice().to_vec());
        let mut cursor = spool.posting_cursor(terms);
        while let Some(row) = cursor.next_edit(&mut work)? {
            let expected_root = u32::from(
                observed
                    .checked_rem(64)
                    .ok_or(IndexError::ResourceLimited)?,
            );
            let expected_reference = if observed.is_multiple_of(2) {
                first
            } else {
                second
            };
            check_eq!(row.root, expected_root);
            check_eq!(row.edit.key, owner(observed));
            check_eq!(row.edit.expected, None);
            check_eq!(row.edit.next, Some(MapValue::Object(expected_reference)));
            observed = observed.checked_add(1).ok_or(IndexError::ResourceLimited)?;
        }
        let frozen = work.usage();
        check!(cursor.next_edit(&mut work)?.is_none());
        check_eq!(work.usage(), frozen);
    }
    check_eq!(page_sizes, vec![64, 64, 1]);
    check_eq!(observed, 129);
    check_eq!(
        range_runs(&spool, &setup)?
            .checked_sub(before)
            .ok_or(IndexError::ResourceLimited)?,
        5
    );
    Ok(())
}

#[test]
fn posting_range_continuation_does_not_skip_following_term_or_late_malformed_row()
-> Result<(), IndexError> {
    let mut setup = budget()?;
    let spool = TextSpool::new(&TestScratch, &mut setup)?;
    let reference = ObjectRef(quanta_index_core::index::Digest([33; 32]));
    for index in 0_u16..65 {
        spool.posting_batch(
            &[(
                b"\0a".to_vec(),
                MapEdit {
                    key: owner(index),
                    expected: None,
                    next: Some(MapValue::Object(reference)),
                },
            )],
            &mut setup,
        )?;
    }
    spool.posting_batch(
        &[(
            b"\0b".to_vec(),
            MapEdit {
                key: owner(100),
                expected: None,
                next: Some(MapValue::Object(reference)),
            },
        )],
        &mut setup,
    )?;
    let mut work = scan(150)?;
    let terms = spool.posting_term_page(None, &mut work)?;
    check_eq!(terms.len(), 2);
    let mut cursor = spool.posting_cursor(terms);
    for index in 0_u16..65 {
        let row = cursor
            .next_edit(&mut work)?
            .ok_or(IndexError::ResourceLimited)?;
        check_eq!(row.root, 0);
        check_eq!(row.edit.key, owner(index));
    }
    let following = cursor
        .next_edit(&mut work)?
        .ok_or(IndexError::ResourceLimited)?;
    check_eq!(following.root, 1);
    check_eq!(following.edit.key, owner(100));
    check!(cursor.next_edit(&mut work)?.is_none());

    // Distinct fixture: 64 valid prefix rows, malformed 65th, then a valid
    // later term. The malformed row must surface at its position, not be
    // skipped by a cross-term range seek.
    let bad = TextSpool::new(&TestScratch, &mut setup)?;
    for index in 0_u16..64 {
        bad.posting_batch(
            &[(
                b"\0a".to_vec(),
                MapEdit {
                    key: owner(index),
                    expected: None,
                    next: Some(MapValue::Object(reference)),
                },
            )],
            &mut setup,
        )?;
    }
    let db = bad.db.scoped(&setup)?;
    let inserted = db
        .execute(
            "INSERT INTO posting(term,k,p,n) VALUES(?1,?2,NULL,zeroblob(31))",
            params![b"\0a".as_slice(), owner(64)],
        )
        .map_err(|error| sql(&error))?;
    check_eq!(inserted, 1);
    drop(db);
    bad.posting_batch(
        &[(
            b"\0b".to_vec(),
            MapEdit {
                key: owner(100),
                expected: None,
                next: Some(MapValue::Object(reference)),
            },
        )],
        &mut setup,
    )?;
    let mut bad_work = scan(150)?;
    let bad_terms = bad.posting_term_page(None, &mut bad_work)?;
    let mut bad_cursor = bad.posting_cursor(bad_terms);
    for index in 0_u16..64 {
        let row = bad_cursor
            .next_edit(&mut bad_work)?
            .ok_or(IndexError::ResourceLimited)?;
        check_eq!(row.edit.key, owner(index));
    }
    check!(matches!(
        bad_cursor.next_edit(&mut bad_work),
        Err(IndexError::CorruptObject(_))
    ));
    check!(matches!(
        bad_cursor.next_edit(&mut bad_work),
        Err(IndexError::CorruptObject(_))
    ));
    Ok(())
}

#[test]
fn posting_range_rejects_forged_term_pages_and_bad_row_types() -> Result<(), IndexError> {
    let mut setup = budget()?;
    let spool = TextSpool::new(&TestScratch, &mut setup)?;
    let reference = ObjectRef(quanta_index_core::index::Digest([34; 32]));
    for term in [b"\0a", b"\0b", b"\0c"] {
        spool.posting_batch(
            &[(
                term.to_vec(),
                MapEdit {
                    key: owner(1),
                    expected: None,
                    next: Some(MapValue::Object(reference)),
                },
            )],
            &mut setup,
        )?;
    }
    // Interior extra SQL term, missing page term, and non-increasing page
    // are independent of the candidate query shape.
    for keys in [
        &[b"\0a".as_slice(), b"\0c".as_slice()][..],
        &[b"\0a".as_slice(), b"\0b".as_slice(), b"\0d".as_slice()][..],
        &[b"\0b".as_slice(), b"\0a".as_slice()][..],
        &[b"\0a".as_slice(), b"\0a".as_slice()][..],
    ] {
        let mut forged = TermPage::new();
        for key in keys {
            forged.push(key, &setup)?;
        }
        let mut work = scan(20)?;
        let mut cursor = spool.posting_cursor(forged);
        // A valid prefix may be observable, but the forged page must never
        // complete successfully or advance into an unmatched later term.
        let mut failed = false;
        for _ in 0..5 {
            match cursor.next_edit(&mut work) {
                Err(IndexError::CorruptObject(_)) => {
                    failed = true;
                    break;
                }
                Ok(Some(_)) => {}
                Ok(None) | Err(_) => break,
            }
        }
        check!(failed);
    }
    let db = spool.db.scoped(&setup)?;
    let changed = db
        .execute(
            "UPDATE posting SET n=42 WHERE term=?1",
            params![b"\0b".as_slice()],
        )
        .map_err(|error| sql(&error))?;
    check_eq!(changed, 1);
    drop(db);
    let mut work = scan(20)?;
    let terms = spool.posting_term_page(None, &mut work)?;
    let mut cursor = spool.posting_cursor(terms);
    check!(cursor.next_edit(&mut work)?.is_some());
    check!(matches!(
        cursor.next_edit(&mut work),
        Err(IndexError::CorruptObject(_))
    ));
    Ok(())
}

#[test]
fn posting_range_emits_first_zero_edit_before_following_term() -> Result<(), IndexError> {
    let mut setup = budget()?;
    let spool = TextSpool::new(&TestScratch, &mut setup)?;
    let db = spool.db.scoped(&setup)?;
    let inserted = db
        .execute(
            "INSERT INTO posting(term,k,p,n) VALUES(?1,?2,NULL,NULL)",
            params![b"\0a".as_slice(), owner(1)],
        )
        .map_err(|error| sql(&error))?;
    check_eq!(inserted, 1);
    drop(db);
    let reference = ObjectRef(quanta_index_core::index::Digest([36; 32]));
    spool.posting_batch(
        &[(
            b"\0b".to_vec(),
            MapEdit {
                key: owner(2),
                expected: None,
                next: Some(MapValue::Object(reference)),
            },
        )],
        &mut setup,
    )?;
    let mut work = scan(10)?;
    let terms = spool.posting_term_page(None, &mut work)?;
    let mut cursor = spool.posting_cursor(terms);
    let zero = cursor
        .next_edit(&mut work)?
        .ok_or(IndexError::ResourceLimited)?;
    check_eq!(zero.root, 0);
    check_eq!(zero.edit.key, owner(1));
    check_eq!(zero.edit.expected, None);
    check_eq!(zero.edit.next, None);
    let following = cursor
        .next_edit(&mut work)?
        .ok_or(IndexError::ResourceLimited)?;
    check_eq!(following.root, 1);
    check_eq!(following.edit.key, owner(2));
    check_eq!(following.edit.next, Some(MapValue::Object(reference)));
    check!(cursor.next_edit(&mut work)?.is_none());
    Ok(())
}

struct CursorMemory {
    used: Arc<AtomicU64>,
    maximum: u64,
}
impl MemoryScopePort for CursorMemory {
    fn reserve(&self, bytes: u64, _lifetime: MemoryLifetime) -> Result<MemoryLease, IndexError> {
        let _old = self
            .used
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |used| {
                used.checked_add(bytes).filter(|sum| *sum <= self.maximum)
            })
            .map_err(|_used| IndexError::ResourceLimited)?;
        Ok(MemoryLease::new(
            bytes,
            Arc::new(Self {
                used: Arc::clone(&self.used),
                maximum: self.maximum,
            }),
        ))
    }
}
impl MemoryLeaseOwner for CursorMemory {
    fn release(&self, bytes: u64) {
        let _old = self.used.fetch_sub(bytes, Ordering::SeqCst);
    }
}

#[test]
fn posting_range_budget_cancel_and_memory_refusal_precede_sql_and_refund() -> Result<(), IndexError>
{
    let mut setup = budget()?;
    let spool = TextSpool::new(&TestScratch, &mut setup)?;
    let reference = ObjectRef(quanta_index_core::index::Digest([35; 32]));
    spool.posting_batch(
        &[(
            b"\0a".to_vec(),
            MapEdit {
                key: owner(1),
                expected: None,
                next: Some(MapValue::Object(reference)),
            },
        )],
        &mut setup,
    )?;
    let before = range_runs(&spool, &setup)?;
    let mut low = scan(0)?;
    let terms = spool.posting_term_page(None, &mut setup)?;
    let mut cursor = spool.posting_cursor(terms);
    check!(matches!(
        cursor.next_edit(&mut low),
        Err(IndexError::ResourceLimited)
    ));
    check_eq!(range_runs(&spool, &setup)?, before);
    let terms = spool.posting_term_page(None, &mut setup)?;
    let mut cancelled = scan(10)?;
    cancelled.request().cancel_handle().cancel();
    let mut cursor = spool.posting_cursor(terms);
    check!(matches!(
        cursor.next_edit(&mut cancelled),
        Err(IndexError::Cancelled)
    ));
    check_eq!(range_runs(&spool, &setup)?, before);

    let used = Arc::new(AtomicU64::new(0));
    let mut denied = scan(10)?;
    denied.bind_memory_scope(Some(MemoryScope::new(Arc::new(CursorMemory {
        used: Arc::clone(&used),
        maximum: 0,
    }))));
    let terms = spool.posting_term_page(None, &mut setup)?;
    let mut cursor = spool.posting_cursor(terms);
    check!(matches!(
        cursor.next_edit(&mut denied),
        Err(IndexError::ResourceLimited)
    ));
    check_eq!(used.load(Ordering::SeqCst), 0);
    // A row-grant refusal may follow a budgeted SQL read. The caller must
    // retain no memory and must never receive a partial edit.

    let mut admitted = scan(10)?;
    admitted.bind_memory_scope(Some(MemoryScope::new(Arc::new(CursorMemory {
        used: Arc::clone(&used),
        maximum: 1_000_000,
    }))));
    let terms = spool.posting_term_page(None, &mut setup)?;
    let mut cursor = spool.posting_cursor(terms);
    let row = cursor
        .next_edit(&mut admitted)?
        .ok_or(IndexError::ResourceLimited)?;
    check!(used.load(Ordering::SeqCst) > 0);
    drop(row);
    drop(cursor);
    check_eq!(used.load(Ordering::SeqCst), 0);
    Ok(())
}

#[test]
fn posting_range_plan_uses_primary_key_without_temp_sort() -> Result<(), IndexError> {
    let mut setup = budget()?;
    let spool = TextSpool::new(&TestScratch, &mut setup)?;
    let db = spool.db.scoped(&setup)?;
    for query in [POSTING_RANGE_FIRST_SQL, POSTING_RANGE_NEXT_SQL] {
        let mut statement = db
            .prepare(&format!("EXPLAIN QUERY PLAN {query}"))
            .map_err(|error| sql(&error))?;
        // Bind each query in its actual first/continuation parameter order.
        let parameters =
            vec![rusqlite::types::Value::Blob(b"\0a".to_vec()); statement.parameter_count()];
        let mut rows = statement
            .query(rusqlite::params_from_iter(parameters))
            .map_err(|error| sql(&error))?;
        let mut details = Vec::new();
        while let Some(row) = rows.next().map_err(|error| sql(&error))? {
            details.push(row.get::<_, String>(3).map_err(|error| sql(&error))?);
        }
        check!(
            details
                .iter()
                .any(|detail| detail.contains("SEARCH posting USING PRIMARY KEY")),
            "{details:?}"
        );
        check!(
            details
                .iter()
                .all(|detail| !detail.contains("USE TEMP B-TREE")),
            "{details:?}"
        );
    }
    Ok(())
}

#[test]
fn posting_range_rejects_empty_owner_key_in_first_term() -> Result<(), IndexError> {
    let mut setup = budget()?;
    let spool = TextSpool::new(&TestScratch, &mut setup)?;
    let db = spool.db.scoped(&setup)?;
    let inserted = db
        .execute(
            "INSERT INTO posting(term,k,p,n) VALUES(?1,x'',NULL,NULL)",
            params![b"\0a".as_slice()],
        )
        .map_err(|error| sql(&error))?;
    check_eq!(inserted, 1);
    drop(db);
    let reference = ObjectRef(quanta_index_core::index::Digest([37; 32]));
    spool.posting_batch(
        &[(
            b"\0b".to_vec(),
            MapEdit {
                key: owner(1),
                expected: None,
                next: Some(MapValue::Object(reference)),
            },
        )],
        &mut setup,
    )?;
    let mut work = scan(10)?;
    let terms = spool.posting_term_page(None, &mut work)?;
    let mut cursor = spool.posting_cursor(terms);
    check!(matches!(
        cursor.next_edit(&mut work),
        Err(IndexError::CorruptObject(_))
    ));
    Ok(())
}
