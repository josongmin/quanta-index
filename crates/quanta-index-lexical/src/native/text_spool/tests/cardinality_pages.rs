use super::*;
use quanta_index_core::index::{
    MemoryLease, MemoryLeaseOwner, MemoryLifetime, MemoryScope, MemoryScopePort,
};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

struct TrackedMemory {
    used: Arc<AtomicU64>,
    maximum: u64,
}

impl MemoryScopePort for TrackedMemory {
    fn reserve(&self, bytes: u64, _lifetime: MemoryLifetime) -> Result<MemoryLease, IndexError> {
        let _previous = self
            .used
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |used| {
                used.checked_add(bytes)
                    .filter(|total| *total <= self.maximum)
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

impl MemoryLeaseOwner for TrackedMemory {
    fn release(&self, bytes: u64) {
        let _previous = self.used.fetch_sub(bytes, Ordering::SeqCst);
    }
}

fn tracked_budget(
    reads: u64,
    maximum: u64,
    used: &Arc<AtomicU64>,
) -> Result<WorkBudget, IndexError> {
    let mut work = scan_budget(reads)?;
    work.bind_memory_scope(Some(MemoryScope::new(Arc::new(TrackedMemory {
        used: Arc::clone(used),
        maximum,
    }))));
    Ok(work)
}

fn scan_budget(reads: u64) -> Result<WorkBudget, IndexError> {
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

fn query_runs(spool: &TextSpool, budget: &WorkBudget) -> Result<i32, IndexError> {
    let db = spool.db.scoped(budget)?;
    let statement = db
        .prepare_cached(POSTING_CARDINALITY_PAGE_SQL)
        .map_err(|error| sql(&error))?;
    Ok(statement.get_status(rusqlite::StatementStatus::Run))
}

#[test]
fn grouped_cardinality_matches_independent_raw_counts_across_page_boundaries()
-> Result<(), IndexError> {
    let mut setup = budget()?;
    let spool = TextSpool::new(&TestScratch, &mut setup)?;
    let db = spool.db.scoped(&setup)?;
    let mut expected = Vec::new();
    for index in 0_u16..129 {
        let key = format!("\0term-{index:03}").into_bytes();
        for (owner, prior, next) in [
            (0_u8, None, Some(b"next".as_slice())),
            (1, Some(b"prior".as_slice()), None),
            (2, Some(b"same".as_slice()), Some(b"same".as_slice())),
        ] {
            let inserted = db
                .execute(
                    "INSERT INTO posting(term,k,p,n) VALUES(?1,?2,?3,?4)",
                    params![key.as_slice(), [owner].as_slice(), prior, next],
                )
                .map_err(|error| sql(&error))?;
            check_eq!(inserted, 1);
        }
        if index.is_multiple_of(3) {
            let inserted = db
                .execute(
                    "INSERT INTO posting(term,k,p,n) VALUES(?1,x'04',NULL,x'01')",
                    params![key.as_slice()],
                )
                .map_err(|error| sql(&error))?;
            check_eq!(inserted, 1);
        }
        let mut raw = db
            .prepare_cached("SELECT p,n FROM posting WHERE term=?1 ORDER BY k")
            .map_err(|error| sql(&error))?;
        let mut rows = raw
            .query(params![key.as_slice()])
            .map_err(|error| sql(&error))?;
        let mut adds = 0_i64;
        let mut removes = 0_i64;
        while let Some(row) = rows.next().map_err(|error| sql(&error))? {
            let p: Option<Vec<u8>> = row.get(0).map_err(|error| sql(&error))?;
            let n: Option<Vec<u8>> = row.get(1).map_err(|error| sql(&error))?;
            if p.is_none() && n.is_some() {
                adds = adds.checked_add(1).ok_or(IndexError::ResourceLimited)?;
            }
            if p.is_some() && n.is_none() {
                removes = removes.checked_add(1).ok_or(IndexError::ResourceLimited)?;
            }
        }
        check_eq!(adds, if index.is_multiple_of(3) { 2 } else { 1 });
        check_eq!(removes, 1);
        expected.push((key, adds, removes));
    }
    drop(db);

    let mut scan = scan_budget(262)?;
    let before = query_runs(&spool, &setup)?;
    let mut after: Option<Vec<u8>> = None;
    let mut observed = 0usize;
    loop {
        let terms = spool.posting_term_page(after.as_deref(), &mut scan)?;
        if terms.is_empty() {
            break;
        }
        after = terms.last().map(|term| term.as_slice().to_vec());
        let before_reads = scan.usage().reads;
        let counts = spool.posting_cardinalities(&terms, &mut scan)?;
        check_eq!(
            scan.usage()
                .reads
                .checked_sub(before_reads)
                .ok_or(IndexError::ResourceLimited)?,
            u64::try_from(terms.len()).map_err(|_error| IndexError::ResourceLimited)?
        );
        if observed == 0 {
            check!(matches!(counts.df(0, 0), Err(IndexError::CorruptObject(_))));
            check!(matches!(
                counts.df(0, u64::MAX),
                Err(IndexError::CorruptObject(_))
            ));
        }
        for (slot, term) in terms.iter().enumerate() {
            let (key, adds, removes) = expected.get(observed).ok_or(IndexError::ResourceLimited)?;
            check_eq!(term.as_slice(), key.as_slice());
            let raw_df = 5_i64
                .checked_sub(*removes)
                .and_then(|df| df.checked_add(*adds))
                .ok_or(IndexError::ResourceLimited)?;
            check_eq!(
                counts.df(slot, 5)?,
                u64::try_from(raw_df).map_err(|_error| IndexError::ResourceLimited)?
            );
            observed = observed.checked_add(1).ok_or(IndexError::ResourceLimited)?;
        }
        check!(matches!(
            counts.df(terms.len(), 5),
            Err(IndexError::ResourceLimited)
        ));
    }
    check_eq!(observed, 129);
    check_eq!(scan.usage().reads, 262);
    check_eq!(
        query_runs(&spool, &setup)?
            .checked_sub(before)
            .ok_or(IndexError::ResourceLimited)?,
        3
    );
    let db = spool.db.scoped(&setup)?;
    let mut plan = db
        .prepare(&format!(
            "EXPLAIN QUERY PLAN {POSTING_CARDINALITY_PAGE_SQL}"
        ))
        .map_err(|error| sql(&error))?;
    let mut rows = plan
        .query(params![b"\0term-000".as_slice(), b"\0term-063".as_slice()])
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
    Ok(())
}

#[test]
fn grouped_cardinality_refuses_wrong_page_and_budget_before_sql() -> Result<(), IndexError> {
    let mut setup = budget()?;
    let spool = TextSpool::new(&TestScratch, &mut setup)?;
    let db = spool.db.scoped(&setup)?;
    for term in [b"\0a", b"\0b", b"\0c"] {
        let inserted = db
            .execute(
                "INSERT INTO posting(term,k,p,n) VALUES(?1,x'01',NULL,x'01')",
                params![term.as_slice()],
            )
            .map_err(|error| sql(&error))?;
        check_eq!(inserted, 1);
    }
    drop(db);
    let empty = TermPage::new();
    let before_empty = query_runs(&spool, &setup)?;
    check!(matches!(
        spool.posting_cardinalities(&empty, &mut scan_budget(0)?),
        Err(IndexError::CorruptObject(_))
    ));
    check_eq!(query_runs(&spool, &setup)?, before_empty);
    let mut wrong = TermPage::new();
    wrong.push(b"\0a", &setup)?;
    wrong.push(b"\0c", &setup)?;
    check!(matches!(
        spool.posting_cardinalities(&wrong, &mut scan_budget(2)?),
        Err(IndexError::CorruptObject(_))
    ));
    let mut missing = TermPage::new();
    missing.push(b"\0a", &setup)?;
    missing.push(b"\0b", &setup)?;
    missing.push(b"\0d", &setup)?;
    check!(matches!(
        spool.posting_cardinalities(&missing, &mut scan_budget(3)?),
        Err(IndexError::CorruptObject(_))
    ));
    let before = query_runs(&spool, &setup)?;
    let mut duplicate = TermPage::new();
    duplicate.push(b"\0a", &setup)?;
    duplicate.push(b"\0a", &setup)?;
    check!(matches!(
        spool.posting_cardinalities(&duplicate, &mut scan_budget(2)?),
        Err(IndexError::CorruptObject(_))
    ));
    check_eq!(query_runs(&spool, &setup)?, before);
    let mut oversized = TermPage::new();
    for index in 0_u8..64 {
        oversized.push(&[0, index], &setup)?;
    }
    check!(matches!(
        oversized.push(b"\0overflow", &setup),
        Err(IndexError::ResourceLimited)
    ));
    let mut ordered = TermPage::new();
    ordered.push(b"\0a", &setup)?;
    ordered.push(b"\0b", &setup)?;
    let used = Arc::new(AtomicU64::new(0));
    let backing = u64::try_from(std::mem::size_of::<[PostingDelta; PAGE]>())
        .map_err(|_error| IndexError::ResourceLimited)?;
    let mut no_memory = tracked_budget(
        2,
        backing.checked_sub(1).ok_or(IndexError::ResourceLimited)?,
        &used,
    )?;
    check!(matches!(
        spool.posting_cardinalities(&ordered, &mut no_memory),
        Err(IndexError::ResourceLimited)
    ));
    check_eq!(no_memory.usage().reads, 0);
    check_eq!(used.load(Ordering::SeqCst), 0);
    check_eq!(query_runs(&spool, &setup)?, before);
    let mut denied = tracked_budget(1, backing, &used)?;
    check!(matches!(
        spool.posting_cardinalities(&ordered, &mut denied),
        Err(IndexError::ResourceLimited)
    ));
    check_eq!(denied.usage().reads, 1);
    check_eq!(used.load(Ordering::SeqCst), 0);
    check_eq!(query_runs(&spool, &setup)?, before);
    let mut cancelled = scan_budget(2)?;
    cancelled.request().cancel_handle().cancel();
    check!(matches!(
        spool.posting_cardinalities(&ordered, &mut cancelled),
        Err(IndexError::Cancelled)
    ));
    check_eq!(query_runs(&spool, &setup)?, before);
    let mut admitted = tracked_budget(2, backing, &used)?;
    let counts = spool.posting_cardinalities(&ordered, &mut admitted)?;
    check_eq!(counts.df(0, 5)?, 6);
    check_eq!(used.load(Ordering::SeqCst), backing);
    drop(counts);
    check_eq!(used.load(Ordering::SeqCst), 0);
    check_eq!(
        query_runs(&spool, &setup)?,
        before.checked_add(1).ok_or(IndexError::ResourceLimited)?
    );
    Ok(())
}
