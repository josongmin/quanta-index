use super::*;
use quanta_index_core::index::{MapValue, ObjectRef};
use quanta_index_core::index::{MemoryLeaseOwner, MemoryScope, MemoryScopePort};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

#[derive(Clone)]
struct SparseReadMemory {
    live: Arc<AtomicU64>,
    maximum: u64,
}

impl MemoryScopePort for SparseReadMemory {
    fn reserve(&self, bytes: u64, _lifetime: MemoryLifetime) -> Result<MemoryLease, IndexError> {
        let _before = self
            .live
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |used| {
                used.checked_add(bytes)
                    .filter(|total| *total <= self.maximum)
            })
            .map_err(|_used| IndexError::ResourceLimited)?;
        Ok(MemoryLease::new(bytes, Arc::new(self.clone())))
    }
}

impl MemoryLeaseOwner for SparseReadMemory {
    fn release(&self, bytes: u64) {
        let _before = self.live.fetch_sub(bytes, Ordering::SeqCst);
    }
}

fn row(term: &[u8], owner: u16, value: MapValue) -> (Vec<u8>, MapEdit) {
    let mut key = vec![0; 30];
    key.extend_from_slice(&owner.to_be_bytes());
    (
        term.to_vec(),
        MapEdit {
            key,
            expected: None,
            next: Some(value),
        },
    )
}
fn object(byte: u8) -> MapValue {
    MapValue::Object(ObjectRef(Digest([byte; 32])))
}

#[test]
fn packed_sparse_two_selected_roots_admit_only_selected_row_backing() -> Result<(), IndexError> {
    let mut setup = budget()?;
    let spool = TextSpool::new(&TestScratch, &mut setup)?;
    let mut rows = Vec::new();
    for (term, owners) in [
        (b"\0a".as_slice(), vec![1_u16, 2]),
        (b"\0b".as_slice(), (3_u16..=11).collect()),
        (b"\0c".as_slice(), vec![12]),
    ] {
        for owner in owners {
            rows.push(row(term, owner, object(1)));
        }
    }
    spool.posting_batch(&rows, &mut setup)?;
    let terms = spool.posting_term_page(None, &mut setup)?;
    let row_bytes = std::mem::size_of::<Option<OrdinalMapEdit>>();
    let selected_capacity = 16_usize
        .checked_mul(row_bytes)
        .ok_or(IndexError::ResourceLimited)?;
    // This is an independent Original memory grant, smaller than one old
    // 512-row backing allocation. It still admits 16 selected rows plus the
    // bounded authenticated packet and cursor heads in this fixed fixture.
    let maximum = 4095_u64;
    check!(
        u64::try_from(selected_capacity).map_err(|_error| IndexError::ResourceLimited)? < maximum
    );
    let live = Arc::new(AtomicU64::new(0));
    let mut scan = WorkBudget::new(
        WorkLimits {
            reads: 1000,
            writes: 0,
            bytes: 2_000_000,
            mutations: 0,
        },
        Instant::now()
            .checked_add(Duration::from_secs(10))
            .ok_or(IndexError::ResourceLimited)?,
    );
    scan.bind_memory_scope(Some(MemoryScope::new(Arc::new(SparseReadMemory {
        live: Arc::clone(&live),
        maximum,
    }))));
    let mut roots = spool
        .posting_root_cursors(&terms, &[true, false, true], &mut scan)
        .inspect_err(|error| {
            eprintln!(
                "selected sparse roots refused: {error}; live {}; {scan:?}",
                live.load(Ordering::SeqCst)
            );
        })?;
    for (slot, gold) in [(0, &[1_u16, 2][..]), (2, &[12_u16][..])] {
        let mut cursor = roots.cursor(
            slot,
            terms
                .get(slot)
                .ok_or(IndexError::ResourceLimited)?
                .as_slice(),
        )?;
        for owner in gold {
            let actual = cursor
                .next_edit(&mut scan)?
                .ok_or(IndexError::ResourceLimited)?;
            let mut expected_key = vec![0; 30];
            expected_key.extend_from_slice(&owner.to_be_bytes());
            check_eq!(actual.root(), 0);
            check_eq!(actual.key(), expected_key);
            check_eq!(actual.expected()?, None);
            check_eq!(actual.next()?, Some(object(1)));
        }
        check!(cursor.next_edit(&mut scan)?.is_none());
    }
    drop(roots);
    check_eq!(live.load(Ordering::SeqCst), 0);
    Ok(())
}

#[test]
fn packed_posting_sparse_prepass_and_dense_fallback_preserve_independent_owner_gold()
-> Result<(), IndexError> {
    let mut work = budget()?;
    let spool = TextSpool::new(&TestScratch, &mut work)?;
    let mut rows = Vec::new();
    for (term, owners) in [
        (b"\0a".as_slice(), vec![1_u16, 2]),
        (b"\0b".as_slice(), (3_u16..=11).collect()),
        (b"\0c".as_slice(), vec![12]),
    ] {
        for owner in owners {
            rows.push(row(term, owner, object(1)));
        }
    }
    spool.posting_batch(&rows, &mut work)?;
    let terms = spool.posting_term_page(None, &mut work)?;
    check_eq!(terms.len(), 3);
    let mut roots = spool.posting_root_cursors(&terms, &[true, false, true], &mut work)?;
    for (slot, expected) in [(0, &[1_u16, 2][..]), (2, &[12_u16][..])] {
        let mut cursor = roots.cursor(
            slot,
            terms
                .get(slot)
                .ok_or(IndexError::ResourceLimited)?
                .as_slice(),
        )?;
        check!(matches!(&cursor, PostingRootCursor::Prefetched { .. }));
        for owner in expected {
            let edit = cursor
                .next_edit(&mut work)?
                .ok_or(IndexError::ResourceLimited)?;
            check_eq!(edit.root(), 0);
            let mut expected_key = vec![0; 30];
            expected_key.extend_from_slice(&owner.to_be_bytes());
            check_eq!(edit.key(), expected_key);
            check_eq!(edit.expected()?, None);
            check_eq!(edit.next()?, Some(object(1)));
        }
        check!(cursor.next_edit(&mut work)?.is_none());
    }
    let mut dense = roots.cursor(
        1,
        terms.get(1).ok_or(IndexError::ResourceLimited)?.as_slice(),
    )?;
    check!(matches!(&dense, PostingRootCursor::Shared(_)));
    for owner in 3_u16..=11 {
        let edit = dense
            .next_edit(&mut work)?
            .ok_or(IndexError::ResourceLimited)?;
        let mut expected_key = vec![0; 30];
        expected_key.extend_from_slice(&owner.to_be_bytes());
        check_eq!(edit.root(), 0);
        check_eq!(edit.key(), expected_key);
        check_eq!(edit.expected()?, None);
        check_eq!(edit.next()?, Some(object(1)));
    }
    check!(dense.next_edit(&mut work)?.is_none());
    Ok(())
}

#[test]
fn packed_posting_129_term_pages_preserve_raw_key_and_ordinal_order() -> Result<(), IndexError> {
    let mut work = budget()?;
    let spool = TextSpool::new(&TestScratch, &mut work)?;
    let mut rows = Vec::new();
    for index in 0_u16..129 {
        let term = format!("\0term-{index:03}");
        rows.push(row(term.as_bytes(), 2, object(2)));
        rows.push(row(term.as_bytes(), 1, object(1)));
    }
    for chunk in rows.chunks(quanta_index_scratch_sqlite::MAX_WRITE_BATCH_ROWS) {
        spool.posting_batch(chunk, &mut work)?;
    }
    let mut seen = 0_u16;
    let mut after = None;
    let mut page_sizes = Vec::new();
    loop {
        let page = spool.posting_term_page(after.as_deref(), &mut work)?;
        if page.is_empty() {
            break;
        }
        let first = seen;
        let page_len = page.len();
        page_sizes.push(page_len);
        for (slot, term) in page.iter().enumerate() {
            let ordinal = first
                .checked_add(u16::try_from(slot).map_err(|_error| IndexError::ResourceLimited)?)
                .ok_or(IndexError::ResourceLimited)?;
            let expected = format!("\0term-{ordinal:03}");
            check_eq!(term.as_slice(), expected.as_bytes());
        }
        let mut cursor = spool.posting_cursor(page);
        for slot in 0..page_len {
            for owner in 1_u16..=2 {
                let edit = cursor
                    .next_edit(&mut work)?
                    .ok_or(IndexError::ResourceLimited)?;
                let mut expected_key = vec![0; 30];
                expected_key.extend_from_slice(&owner.to_be_bytes());
                check_eq!(
                    edit.root(),
                    u32::try_from(slot).map_err(|_error| IndexError::ResourceLimited)?
                );
                check_eq!(edit.key(), expected_key);
                check_eq!(edit.expected()?, None);
                check_eq!(
                    edit.next()?,
                    Some(object(
                        u8::try_from(owner).map_err(|_error| IndexError::ResourceLimited)?
                    ))
                );
            }
        }
        check!(cursor.next_edit(&mut work)?.is_none());
        seen = seen
            .checked_add(u16::try_from(page_len).map_err(|_error| IndexError::ResourceLimited)?)
            .ok_or(IndexError::ResourceLimited)?;
        after = Some(
            format!(
                "\0term-{:03}",
                seen.checked_sub(1).ok_or(IndexError::ResourceLimited)?
            )
            .into_bytes(),
        );
    }
    check_eq!(seen, 129);
    check_eq!(page_sizes, vec![64, 64, 1]);
    Ok(())
}

#[test]
fn packed_posting_forged_terms_and_cancelled_cursor_refuse() -> Result<(), IndexError> {
    let mut work = budget()?;
    let spool = TextSpool::new(&TestScratch, &mut work)?;
    spool.posting_batch(
        &[row(b"\0a", 1, object(1)), row(b"\0b", 2, object(2))],
        &mut work,
    )?;
    let mut forged = TermPage::new();
    forged.push(b"\0a", &work)?;
    forged.push(b"\0missing", &work)?;
    let mut cursor = spool.posting_cursor(forged);
    check!(cursor.next_edit(&mut work)?.is_some());
    check!(matches!(
        cursor.next_edit(&mut work),
        Err(IndexError::CorruptObject(_))
    ));
    let terms = spool.posting_term_page(None, &mut work)?;
    let mut cancelled = spool.posting_cursor(terms);
    let mut denied = budget()?;
    denied.request().cancel_handle().cancel();
    check!(matches!(
        cancelled.next_edit(&mut denied),
        Err(IndexError::Cancelled)
    ));
    check!(cancelled.next_edit(&mut work)?.is_some());
    Ok(())
}

#[test]
fn packed_paired_case_pages_preserve_64_65_boundary_and_complete_census() -> Result<(), IndexError>
{
    check_paired_pages_with_packet_reuse(true)?;
    check_paired_pages_with_packet_reuse(false)
}

fn check_paired_pages_with_packet_reuse(funded: bool) -> Result<(), IndexError> {
    let mut work = budget()?;
    let live = Arc::new(AtomicU64::new(0));
    if funded {
        work.bind_memory_scope(Some(MemoryScope::new(Arc::new(SparseReadMemory {
            live: Arc::clone(&live),
            maximum: 8 * 1024 * 1024,
        }))));
    }
    let spool = TextSpool::new(&TestScratch, &mut work)?;
    let mut expected = Vec::new();
    for index in 0_u16..63 {
        expected.push((format!("\0a{index:03}").into_bytes(), index));
    }
    expected.extend([
        (b"\0m".to_vec(), 63),
        (b"\x01m".to_vec(), 64),
        (b"\x01n".to_vec(), 65),
        ("\0é".as_bytes().to_vec(), 66),
    ]);
    let rows = expected
        .iter()
        .map(|(term, index)| row(term, *index, object(81)))
        .collect::<Vec<_>>();
    for chunk in rows.chunks(quanta_index_scratch_sqlite::MAX_WRITE_BATCH_ROWS) {
        spool.posting_batch(chunk, &mut work)?;
    }
    let mut pages = PostingCasePages::new();
    let mut observed = Vec::new();
    let mut sizes = Vec::new();
    let mut packet_queries = Vec::new();
    let mut next_stage_memory = None;
    loop {
        let page = pages.next(&spool, &mut work)?;
        if page.is_empty() {
            break;
        }
        sizes.push(page.len());
        if sizes.len() == 1 {
            check_eq!(page.len(), 63);
            check!(!page.case_ranges);
        }
        if sizes.len() == 2 {
            check_eq!(page.len(), 4);
            check!(page.case_ranges);
        }
        let keys = page
            .iter()
            .map(|term| term.as_slice().to_vec())
            .collect::<Vec<_>>();
        let cardinalities = spool.posting_cardinalities(&page, &mut work)?;
        let (before, packet_bytes) = spool.posting_packet_read_probe(&work)?;
        check!(packet_bytes > 0);
        let mut cursor = pages.posting_cursor(&spool, page);
        for (slot, term) in keys.iter().enumerate() {
            check_eq!(cardinalities.df(slot, 0)?, 1);
            let edit = cursor
                .next_edit(&mut work)?
                .ok_or(IndexError::ResourceLimited)?;
            check_eq!(
                edit.root(),
                u32::try_from(slot).map_err(|_error| IndexError::ResourceLimited)?
            );
            let expected_owner = expected
                .iter()
                .find(|(key, _)| key == term)
                .map(|(_, index)| *index)
                .ok_or(IndexError::ResourceLimited)?;
            let mut expected_key = vec![0; 30];
            expected_key.extend_from_slice(&expected_owner.to_be_bytes());
            check_eq!(edit.key(), expected_key);
            check_eq!(edit.expected()?, None);
            check_eq!(edit.next()?, Some(object(81)));
            observed.push(term.clone());
        }
        // Count queries for existing payloads before the final EOF probe.
        packet_queries.push(spool.posting_packet_read_probe(&work)?.0 - before);
        check!(cursor.next_edit(&mut work)?.is_none());
        drop(cursor);
        if sizes.len() == 1 {
            // The first term window is complete. Its normal top-table write
            // advances the SQLite connection's change counter.
            if funded {
                // A sealed spool no longer needs its initial run writer.
                // Leave 512 KiB for the reader and the rest of the page
                // pipeline while running the actual top write and next page.
                next_stage_memory = Some(
                    work.reserve_memory(7680 * 1024, MemoryLifetime::Temporary)
                        .map_err(|error| {
                            IndexError::BackendViolation(format!(
                                "sealed writer blocks the next stage: {error}; live {}",
                                live.load(Ordering::SeqCst)
                            ))
                        })?,
                );
            }
            let epoch = spool.db.change_count()?;
            spool.top_batch(
                &[(
                    0,
                    MapEdit {
                        key: b"\0a000".to_vec(),
                        expected: None,
                        next: Some(object(82)),
                    },
                )],
                &mut work,
            )?;
            check!(spool.db.change_count()? > epoch);
        }
    }
    drop(next_stage_memory);
    check_eq!(sizes, vec![63, 4]);
    observed.sort();
    let mut fixed = expected
        .into_iter()
        .map(|(term, _)| term)
        .collect::<Vec<_>>();
    fixed.sort();
    check_eq!(observed, fixed);
    check_eq!(packet_queries, vec![1, i32::from(!funded)]);

    let terms = spool.posting_term_page(None, &mut work)?;
    let db = spool.db.scoped(&work)?;
    let changed = db
        .execute(
            "UPDATE posting_packet SET payload=zeroblob(length(payload))",
            [],
        )
        .map_err(|error| sql(&error))?;
    check_eq!(changed, 1);
    drop(db);
    let before = spool.posting_packet_read_probe(&work)?.0;
    let mut stale = pages.posting_cursor(&spool, terms);
    check!(matches!(
        stale.next_edit(&mut work),
        Err(IndexError::CorruptObject(message))
            if message == "posting scratch packet: sealed posting scratch source changed"
    ));
    check_eq!(spool.posting_packet_read_probe(&work)?.0, before);
    drop(stale);
    drop(pages);
    drop(spool);
    check_eq!(live.load(Ordering::SeqCst), 0);
    Ok(())
}

#[test]
fn packed_case_metadata_rejects_unknown_surface_with_valid_payload_digest_before_page()
-> Result<(), IndexError> {
    use sha2::{Digest as _, Sha256};
    let mut work = budget()?;
    let spool = TextSpool::new(&TestScratch, &mut work)?;
    spool.posting_batch(&[row(b"\0valid", 1, object(4))], &mut work)?;
    let page = spool.posting_term_page(None, &mut work)?;
    check_eq!(page.len(), 1);
    let db = spool.db.scoped(&work)?;
    let mut wire: Vec<u8> = db
        .query_row(
            "SELECT payload FROM posting_term_packet WHERE seq=0",
            [],
            |r| r.get(0),
        )
        .map_err(|e| sql(&e))?;
    let surface = wire.get_mut(44).ok_or(IndexError::ResourceLimited)?;
    check_eq!(*surface, 0);
    *surface = 2;
    let digest = Sha256::digest(wire.get(42..).ok_or(IndexError::ResourceLimited)?);
    wire.get_mut(10..42)
        .ok_or(IndexError::ResourceLimited)?
        .copy_from_slice(&digest);
    let changed = db
        .execute(
            "UPDATE posting_term_packet SET payload=?1,first_term=?2,last_term=?2 WHERE seq=0",
            params![wire, b"\x02valid".as_slice()],
        )
        .map_err(|e| sql(&e))?;
    check_eq!(changed, 1);
    drop(db);
    let mut pages = PostingCasePages::new();
    check!(matches!(
        pages.next(&spool, &mut work),
        Err(IndexError::CorruptObject(_))
    ));
    Ok(())
}
