use super::*;

fn row(
    term: &[u8],
    owner: u8,
    expected: Option<MapValue>,
    next: Option<MapValue>,
) -> (Vec<u8>, MapEdit) {
    (
        term.to_vec(),
        MapEdit {
            key: vec![owner; 32],
            expected,
            next,
        },
    )
}

#[test]
fn packed_cardinality_matches_fixed_add_remove_noop_gold_across_129_terms() -> Result<(), IndexError>
{
    let mut work = budget()?;
    let spool = TextSpool::new(&TestScratch, &mut work)?;
    let before = MapValue::Object(ObjectRef(Digest([2; 32])));
    let after = MapValue::Object(ObjectRef(Digest([3; 32])));
    let mut rows = Vec::new();
    for index in 0_u16..129 {
        let term = format!("\0term-{index:03}");
        rows.push(row(term.as_bytes(), 1, None, Some(after)));
        rows.push(row(term.as_bytes(), 2, Some(before), None));
        rows.push(row(term.as_bytes(), 3, Some(before), Some(before)));
        if index == 0 {
            rows.push(row(term.as_bytes(), 4, None, Some(after)));
        }
    }
    for chunk in rows.chunks(quanta_index_scratch_sqlite::MAX_WRITE_BATCH_ROWS) {
        spool.posting_batch(chunk, &mut work)?;
    }
    let mut observed = 0_u16;
    let mut after_key = None;
    loop {
        let terms = spool.posting_term_page(after_key.as_deref(), &mut work)?;
        if terms.is_empty() {
            break;
        }
        check!(terms.len() <= 64);
        let counts = spool.posting_cardinalities(&terms, &mut work)?;
        for (slot, term) in terms.iter().enumerate() {
            let expected = format!("\0term-{observed:03}");
            check_eq!(term.as_slice(), expected.as_bytes());
            check_eq!(counts.df(slot, 5)?, if observed == 0 { 6 } else { 5 });
            observed = observed.checked_add(1).ok_or(IndexError::ResourceLimited)?;
        }
        check!(matches!(
            counts.df(terms.len(), 5),
            Err(IndexError::ResourceLimited)
        ));
        after_key = terms.last().map(|key| key.as_slice().to_vec());
    }
    check_eq!(observed, 129);
    Ok(())
}

#[test]
fn packed_cardinality_reuses_authenticated_term_page_without_packet_reread()
-> Result<(), IndexError> {
    let mut work = budget()?;
    let spool = TextSpool::new(&TestScratch, &mut work)?;
    let value = MapValue::Object(ObjectRef(Digest([7; 32])));
    let rows = (0_u16..129)
        .map(|index| {
            let term = format!("\0term-{index:03}");
            row(term.as_bytes(), 1, None, Some(value))
        })
        .collect::<Vec<_>>();
    for chunk in rows.chunks(quanta_index_scratch_sqlite::MAX_WRITE_BATCH_ROWS) {
        spool.posting_batch(chunk, &mut work)?;
    }

    let terms = spool.posting_term_page(None, &mut work)?;
    check_eq!(terms.len(), 64);
    let before = work.usage();
    let counts = spool.posting_cardinalities(&terms, &mut work)?;
    let after = work.usage();
    // One fixed logical read per selected term remains. The authenticated
    // packet's counts need no second SQL read or packet-body copy. The new
    // integrity pass is charged: domain36 + framing9 + 64*(key length9+24).
    check_eq!(after.reads - before.reads, 64);
    check_eq!(after.bytes - before.bytes, 2157);
    for slot in 0..64 {
        let expected = format!("\0term-{slot:03}");
        check_eq!(terms[slot].as_slice(), expected.as_bytes());
        check_eq!(counts.df(slot, 0)?, 1);
    }

    let proof = counts.dense_prepass_proof();
    let mut cursor = spool.posting_cursor(terms);
    cursor.check_cardinalities(&proof, &work)?;
    for slot in 0..64_u32 {
        let edit = cursor.next_edit(&mut work)?.ok_or(IndexError::Conflict)?;
        check_eq!(edit.root(), slot);
        check_eq!(edit.key(), &[1; 32]);
        check_eq!(edit.expected()?, None);
        check_eq!(edit.next()?, Some(value));
    }
    check!(cursor.next_edit(&mut work)?.is_none());
    cursor.check_cardinalities(&proof, &work)?;
    Ok(())
}

#[test]
fn sealed_term_page_read_charges_only_packet_lookup_and_count_integrity() -> Result<(), IndexError>
{
    let mut work = budget()?;
    let spool = TextSpool::new(&TestScratch, &mut work)?;
    let value = MapValue::Object(ObjectRef(Digest([13; 32])));
    let rows = (0_u16..129)
        .map(|index| {
            let term = format!("\0term-{index:03}");
            row(term.as_bytes(), 1, None, Some(value))
        })
        .collect::<Vec<_>>();
    for chunk in rows.chunks(quanta_index_scratch_sqlite::MAX_WRITE_BATCH_ROWS) {
        spool.posting_batch(chunk, &mut work)?;
    }

    // The first read seals the scratch source. Measure only a subsequent page
    // read so producer finalization cannot enter the consumer Work delta.
    let first = spool.posting_term_page(None, &mut work)?;
    check_eq!(first.len(), 64);
    let before = work.usage();
    let page = spool.posting_term_page(None, &mut work)?;
    let after = work.usage();
    check_eq!(after.reads - before.reads, 1);
    // One SQL packet lookup plus the authenticated count digest:
    // domain36 + framing9 + 64*(nine key bytes + 24 delta bytes).
    check_eq!(after.bytes - before.bytes, 4096 + 2157);
    check_eq!(page.len(), 64);
    for (slot, key) in page.iter().enumerate() {
        let expected = format!("\0term-{slot:03}");
        check_eq!(key.as_slice(), expected.as_bytes());
    }
    let counts = spool.posting_cardinalities(&page, &mut work)?;
    for slot in 0..64 {
        check_eq!(counts.df(slot, 0)?, 1);
    }
    Ok(())
}

#[test]
fn manual_cardinality_page_across_packet_fence_charges_two_lookups() -> Result<(), IndexError> {
    let mut work = budget()?;
    let spool = TextSpool::new(&TestScratch, &mut work)?;
    let value = MapValue::Object(ObjectRef(Digest([14; 32])));
    let rows = (0_u16..129)
        .map(|index| {
            let term = format!("\0term-{index:03}");
            row(term.as_bytes(), 1, None, Some(value))
        })
        .collect::<Vec<_>>();
    for chunk in rows.chunks(quanta_index_scratch_sqlite::MAX_WRITE_BATCH_ROWS) {
        spool.posting_batch(chunk, &mut work)?;
    }
    let _sealed = spool.posting_term_page(None, &mut work)?;

    // A manually constructed page has no authenticated count sidecar. Its two
    // adjacent keys straddle the fixed 64-term physical packet boundary.
    let mut page = TermPage::new();
    page.push(b"\0term-063", &work)?;
    page.push(b"\0term-064", &work)?;
    let before = work.usage();
    let counts = spool.posting_cardinalities(&page, &mut work)?;
    let after = work.usage();
    check_eq!(after.reads - before.reads, 4); // Two logical terms and two SQL packets.
    check_eq!(after.bytes - before.bytes, 2 * 4096);
    check_eq!(counts.df(0, 0)?, 1);
    check_eq!(counts.df(1, 0)?, 1);
    Ok(())
}

#[test]
fn packed_cardinality_rejects_forged_page_and_original_read_refusal() -> Result<(), IndexError> {
    let mut work = budget()?;
    let spool = TextSpool::new(&TestScratch, &mut work)?;
    let value = MapValue::Object(ObjectRef(Digest([4; 32])));
    spool.posting_batch(
        &[
            row(b"\0a", 1, None, Some(value)),
            row(b"\0b", 2, Some(value), None),
        ],
        &mut work,
    )?;
    let terms = spool.posting_term_page(None, &mut work)?;
    let mut no_reads = WorkBudget::new(
        WorkLimits {
            reads: 0,
            writes: 1000,
            bytes: 10_000_000,
            mutations: 1000,
        },
        Instant::now()
            .checked_add(Duration::from_secs(10))
            .ok_or(IndexError::ResourceLimited)?,
    );
    check!(matches!(
        spool.posting_cardinalities(&terms, &mut no_reads),
        Err(IndexError::ResourceLimited)
    ));
    let mut empty = TermPage::new();
    check!(matches!(
        spool.posting_cardinalities(&empty, &mut work),
        Err(IndexError::CorruptObject(_))
    ));
    empty.push(b"\0z", &work)?;
    check!(matches!(
        spool.posting_cardinalities(&empty, &mut work),
        Err(IndexError::CorruptObject(_))
    ));
    let mut duplicate = TermPage::new();
    duplicate.push(b"\0a", &work)?;
    duplicate.push(b"\0a", &work)?;
    check!(matches!(
        spool.posting_cardinalities(&duplicate, &mut work),
        Err(IndexError::CorruptObject(_))
    ));
    let counts = spool.posting_cardinalities(&terms, &mut work)?;
    check_eq!(counts.df(0, 2)?, 3);
    check_eq!(counts.df(1, 2)?, 1);
    Ok(())
}

#[test]
fn packed_cardinality_rejects_foreign_count_source_with_identical_keys() -> Result<(), IndexError> {
    let mut work = budget()?;
    let first = TextSpool::new(&TestScratch, &mut work)?;
    let second = TextSpool::new(&TestScratch, &mut work)?;
    let value = MapValue::Object(ObjectRef(Digest([8; 32])));
    first.posting_batch(&[row(b"\0same", 1, None, Some(value))], &mut work)?;
    second.posting_batch(
        &[
            row(b"\0same", 1, None, Some(value)),
            row(b"\0same", 2, None, Some(value)),
        ],
        &mut work,
    )?;
    let first_page = first.posting_term_page(None, &mut work)?;
    let second_page = second.posting_term_page(None, &mut work)?;
    check_eq!(first_page.len(), 1);
    check_eq!(first_page[0].as_slice(), second_page[0].as_slice());
    check_eq!(
        first
            .posting_cardinalities(&first_page, &mut work)?
            .df(0, 0)?,
        1
    );
    check_eq!(
        second
            .posting_cardinalities(&second_page, &mut work)?
            .df(0, 0)?,
        2
    );
    check!(matches!(
        second.posting_cardinalities(&first_page, &mut work),
        Err(IndexError::CorruptObject(_))
    ));
    Ok(())
}

#[test]
fn packed_cardinality_rejects_count_only_page_tamper() -> Result<(), IndexError> {
    let mut work = budget()?;
    let spool = TextSpool::new(&TestScratch, &mut work)?;
    let value = MapValue::Object(ObjectRef(Digest([9; 32])));
    spool.posting_batch(&[row(b"\0same", 1, None, Some(value))], &mut work)?;
    let mut page = spool.posting_term_page(None, &mut work)?;
    check_eq!(page[0].as_slice(), b"\0same");
    check_eq!(spool.posting_cardinalities(&page, &mut work)?.df(0, 0)?, 1);
    page.counts
        .as_mut()
        .ok_or(IndexError::ResourceLimited)?
        .deltas[0]
        .added = 2;
    check_eq!(page[0].as_slice(), b"\0same");
    check!(matches!(
        spool.posting_cardinalities(&page, &mut work),
        Err(IndexError::CorruptObject(_))
    ));
    Ok(())
}

#[test]
fn packed_cardinality_rejects_stale_sql_even_if_packet_payload_is_unchanged()
-> Result<(), IndexError> {
    let mut work = budget()?;
    let spool = TextSpool::new(&TestScratch, &mut work)?;
    let value = MapValue::Object(ObjectRef(Digest([10; 32])));
    spool.posting_batch(&[row(b"\0same", 1, None, Some(value))], &mut work)?;
    let page = spool.posting_term_page(None, &mut work)?;
    let db = spool.db.scoped(&work)?;
    check_eq!(
        db.execute(
            "UPDATE posting_term_packet SET payload=payload WHERE seq=0",
            []
        )
        .map_err(|error| sql(&error))?,
        1
    );
    drop(db);
    check!(matches!(
        spool.posting_cardinalities(&page, &mut work),
        Err(IndexError::CorruptObject(_))
    ));
    Ok(())
}

#[test]
fn packed_cardinality_accepts_top_writes_but_never_launders_unexpected_sql()
-> Result<(), IndexError> {
    let mut work = budget()?;
    let spool = TextSpool::new(&TestScratch, &mut work)?;
    let value = MapValue::Object(ObjectRef(Digest([11; 32])));
    spool.posting_batch(&[row(b"\0same", 1, None, Some(value))], &mut work)?;
    let page = spool.posting_term_page(None, &mut work)?;
    spool.top_batch(
        &[(
            0,
            MapEdit {
                key: b"\0top-1".to_vec(),
                expected: None,
                next: Some(value),
            },
        )],
        &mut work,
    )?;
    check_eq!(spool.posting_cardinalities(&page, &mut work)?.df(0, 0)?, 1);
    let db = spool.db.scoped(&work)?;
    check_eq!(
        db.execute(
            "UPDATE posting_term_packet SET payload=payload WHERE seq=0",
            []
        )
        .map_err(|error| sql(&error))?,
        1
    );
    drop(db);
    let before = spool.db.change_count()?;
    check!(matches!(
        spool.top_batch(
            &[(
                0,
                MapEdit {
                    key: b"\0top-2".to_vec(),
                    expected: None,
                    next: Some(value),
                }
            )],
            &mut work
        ),
        Err(IndexError::CorruptObject(_))
    ));
    check_eq!(spool.db.change_count()?, before);
    check!(matches!(
        spool.posting_cardinalities(&page, &mut work),
        Err(IndexError::CorruptObject(_))
    ));
    Ok(())
}

#[test]
fn packed_paired_cardinality_keeps_case_counts_sorted_across_pages_and_top_writes()
-> Result<(), IndexError> {
    let mut work = budget()?;
    let spool = TextSpool::new(&TestScratch, &mut work)?;
    let value = MapValue::Object(ObjectRef(Digest([12; 32])));
    let mut rows = Vec::new();
    for index in 0_u16..80 {
        let term = format!("term-{index:03}");
        let mut sensitive = vec![0];
        sensitive.extend_from_slice(term.as_bytes());
        let mut folded = vec![1];
        folded.extend_from_slice(term.as_bytes());
        rows.push(row(&sensitive, 1, None, Some(value)));
        rows.push(row(&folded, 1, None, Some(value)));
        rows.push(row(&folded, 2, None, Some(value)));
    }
    for chunk in rows.chunks(quanta_index_scratch_sqlite::MAX_WRITE_BATCH_ROWS) {
        spool.posting_batch(chunk, &mut work)?;
    }

    let mut pages = PostingCasePages::new();
    for (page_number, pairs) in [(0_u16, 32_usize), (1, 32), (2, 16)] {
        let terms = pages.next(&spool, &mut work)?;
        check_eq!(terms.len(), pairs * 2);
        check_eq!(terms.case_cut()?, Some(pairs));
        // A legitimate Top write changes the scratch connection but preserves
        // the authenticated posting source for this resident page and the next.
        let top_key = format!("\0top-{page_number}").into_bytes();
        spool.top_batch(
            &[(
                0,
                MapEdit {
                    key: top_key,
                    expected: None,
                    next: Some(value),
                },
            )],
            &mut work,
        )?;
        let counts = spool.posting_cardinalities(&terms, &mut work)?;
        for slot in 0..pairs {
            let index = u32::from(page_number) * 32
                + u32::try_from(slot).map_err(|_| IndexError::ResourceLimited)?;
            let term = format!("term-{index:03}");
            let mut sensitive = vec![0];
            sensitive.extend_from_slice(term.as_bytes());
            let mut folded = vec![1];
            folded.extend_from_slice(term.as_bytes());
            check_eq!(terms[slot].as_slice(), sensitive.as_slice());
            check_eq!(terms[pairs + slot].as_slice(), folded.as_slice());
            check_eq!(counts.df(slot, 0)?, 1);
            check_eq!(counts.df(pairs + slot, 0)?, 2);
        }
    }
    check!(pages.next(&spool, &mut work)?.is_empty());
    let mut top = spool.top_cursor();
    for page_number in 0_u16..3 {
        let edit = top.next_edit(&mut work)?.ok_or(IndexError::Conflict)?;
        check_eq!(edit.root(), 0);
        let expected_key = format!("\0top-{page_number}");
        check_eq!(edit.key(), expected_key.as_bytes());
        check_eq!(edit.expected()?, None);
        check_eq!(edit.next()?, Some(value));
    }
    check!(top.next_edit(&mut work)?.is_none());
    Ok(())
}
