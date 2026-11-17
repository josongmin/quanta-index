use super::*;
use quanta_index_core::index::{
    MemoryLease, MemoryLeaseOwner, MemoryLifetime, MemoryScope, MemoryScopePort,
};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

fn paired_rows() -> [(Vec<u8>, MapEdit); 2] {
    let value = MapValue::Object(ObjectRef(Digest([0x5a; 32])));
    [b"\0same".as_slice(), b"\x01same".as_slice()].map(|term| {
        (
            term.to_vec(),
            MapEdit {
                key: vec![0x11; 32],
                expected: None,
                next: Some(value),
            },
        )
    })
}

fn final_pair(
    spool: &TextSpool,
    work: &mut WorkBudget,
) -> Result<(PostingCasePages, TermPage), IndexError> {
    spool.posting_batch(&paired_rows(), work)?;
    let mut pages = PostingCasePages::new();
    let page = pages.next(spool, work)?;
    check_eq!(page.len(), 2);
    check_eq!(page[0].as_slice(), b"\0same");
    check_eq!(page[1].as_slice(), b"\x01same");
    check_eq!(page.case_cut()?, Some(1));
    let counts = spool.posting_cardinalities(&page, work)?;
    check_eq!(counts.df(0, 0)?, 1);
    check_eq!(counts.df(1, 0)?, 1);
    // The first next() refills both consumed cursors before returning this page,
    // so both EOF flags are already cached at the next call boundary.
    Ok((pages, page))
}

#[test]
fn paired_page_reuses_untouched_case_count_seals() -> Result<(), IndexError> {
    let mut work = budget()?;
    let spool = TextSpool::new(&TestScratch, &mut work)?;
    spool.posting_batch(&paired_rows(), &mut work)?;
    spool.postings.seal(&spool, &mut work)?;

    let before = work.usage();
    let mut pages = PostingCasePages::new();
    let page = pages.next(&spool, &mut work)?;
    let after = work.usage();
    check_eq!(after.reads - before.reads, 5);
    // Five SQL lookups, two producer count seals, one output count seal,
    // and three real five-byte cursor/key copies. Untouched producer seals
    // need no second integrity hash while joining the paired page.
    check_eq!(after.bytes - before.bytes, 20_746);
    check_eq!(page.len(), 2);
    check_eq!(page[0].as_slice(), b"\0same");
    check_eq!(page[1].as_slice(), b"\x01same");
    check_eq!(page.case_cut()?, Some(1));
    let counts = spool.posting_cardinalities(&page, &mut work)?;
    check_eq!(counts.df(0, 0)?, 1);
    check_eq!(counts.df(1, 0)?, 1);
    Ok(())
}

#[test]
fn paired_eof_rejects_stale_sql_even_when_packet_payload_is_unchanged() -> Result<(), IndexError> {
    let mut work = budget()?;
    let spool = TextSpool::new(&TestScratch, &mut work)?;
    let (mut pages, _page) = final_pair(&spool, &mut work)?;
    let db = spool.db.scoped(&work)?;
    check_eq!(
        db.execute(
            "UPDATE posting_term_packet SET payload=payload WHERE seq=0",
            [],
        )
        .map_err(|error| sql(&error))?,
        1
    );
    drop(db);
    check!(matches!(
        pages.next(&spool, &mut work),
        Err(IndexError::CorruptObject(_))
    ));
    Ok(())
}

#[derive(Clone, Default)]
struct LiveMemory {
    bytes: Arc<AtomicU64>,
    reserves: Arc<AtomicU64>,
}

impl MemoryScopePort for LiveMemory {
    fn reserve(&self, bytes: u64, _lifetime: MemoryLifetime) -> Result<MemoryLease, IndexError> {
        let _previous = self.bytes.fetch_add(bytes, Ordering::SeqCst);
        let _previous = self.reserves.fetch_add(1, Ordering::SeqCst);
        Ok(MemoryLease::new(bytes, Arc::new(self.clone())))
    }
}

impl MemoryLeaseOwner for LiveMemory {
    fn release(&self, bytes: u64) {
        let _previous = self.bytes.fetch_sub(bytes, Ordering::SeqCst);
    }
}

#[test]
fn paired_eof_accepts_top_write_without_new_work_or_backing() -> Result<(), IndexError> {
    let mut work = budget()?;
    let memory = LiveMemory::default();
    work.bind_memory_scope(Some(MemoryScope::new(Arc::new(memory.clone()))));
    let spool = TextSpool::new(&TestScratch, &mut work)?;
    let (mut pages, _page) = final_pair(&spool, &mut work)?;
    let value = MapValue::Object(ObjectRef(Digest([0x5a; 32])));
    spool.top_batch(
        &[(
            0,
            MapEdit {
                key: b"\0top-after-pair".to_vec(),
                expected: None,
                next: Some(value),
            },
        )],
        &mut work,
    )?;
    let before = work.usage();
    let live_before = memory.bytes.load(Ordering::SeqCst);
    let reserves_before = memory.reserves.load(Ordering::SeqCst);
    check!(live_before > 0);
    for _ in 0..2 {
        check!(pages.next(&spool, &mut work)?.is_empty());
        check_eq!(work.usage(), before);
        check_eq!(memory.bytes.load(Ordering::SeqCst), live_before);
        check_eq!(memory.reserves.load(Ordering::SeqCst), reserves_before);
    }
    Ok(())
}
