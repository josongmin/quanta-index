//! Independent observable oracles for the one disposable packed posting stream.
#[path = "ordered_packets.rs"]
mod ordered_packets;
#[path = "packet_read_probe.rs"]
mod packet_read_probe;

use super::*;
use quanta_index_core::index::{MemoryLeaseOwner, MemoryScope, MemoryScopePort};
use std::sync::Arc;

fn edit(
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
fn object(byte: u8) -> MapValue {
    MapValue::Object(ObjectRef(Digest([byte; 32])))
}

#[test]
fn packed_posting_case_order_cas_and_noop_match_fixed_gold() -> Result<(), IndexError> {
    let mut work = budget()?;
    let spool = TextSpool::new(&TestScratch, &mut work)?;
    let rows = [
        edit(b"\x01unicode", 3, None, Some(object(9))),
        edit(b"\x00A", 2, Some(object(4)), None),
        edit(b"\x00A", 1, None, Some(object(3))),
        edit("\x00漢字".as_bytes(), 7, Some(object(8)), Some(object(8))),
    ];
    spool.posting_batch(&rows[..2], &mut work)?;
    spool.posting_batch(&rows[2..], &mut work)?;
    let first = spool.posting_term_page(None, &mut work)?;
    let fixed = [
        b"\x00A".as_slice(),
        "\x00漢字".as_bytes(),
        b"\x01unicode".as_slice(),
    ];
    check_eq!(first.len(), fixed.len());
    for (observed, expected) in first.iter().zip(fixed) {
        check_eq!(observed.as_slice(), expected);
    }
    let mut cursor = spool.posting_cursor(first);
    for (root, owner, prior, next) in [
        (0_u32, 1_u8, None, Some(object(3))),
        (0, 2, Some(object(4)), None),
        (1, 7, Some(object(8)), Some(object(8))),
        (2, 3, None, Some(object(9))),
    ] {
        let observed = cursor
            .next_edit(&mut work)?
            .ok_or(IndexError::ResourceLimited)?;
        check_eq!(observed.root(), root);
        check_eq!(observed.key(), vec![owner; 32]);
        check_eq!(observed.expected()?, prior);
        check_eq!(observed.next()?, next);
    }
    check!(cursor.next_edit(&mut work)?.is_none());
    let stable = work.usage();
    check!(cursor.next_edit(&mut work)?.is_none());
    check_eq!(work.usage(), stable);
    let mut case = PostingCasePages::new();
    let page = case.next(&spool, &mut work)?;
    check_eq!(page.len(), 3);
    check!(case.next(&spool, &mut work)?.is_empty());
    Ok(())
}

#[test]
fn packed_range_seek_does_not_materialize_rows_before_the_start() -> Result<(), IndexError> {
    let mut work = budget()?;
    let spool = TextSpool::new(&TestScratch, &mut work)?;
    let rows: Vec<_> = (0..60_u8)
        .map(|owner| {
            let term = format!("\0{owner:02}-{}", "x".repeat(500));
            edit(term.as_bytes(), owner, None, Some(object(owner)))
        })
        .collect();
    spool.posting_batch(&rows, &mut work)?;
    let terms = spool.posting_term_page(None, &mut work)?;
    let first = terms.first().ok_or(IndexError::ResourceLimited)?.as_slice();
    let last = terms.last().ok_or(IndexError::ResourceLimited)?.as_slice();
    let mut verify_one = |start: &[u8], expected_owner: u8| {
        let mut range = PackedRangeCursor::new(&spool, start, &mut work)?;
        let before = work.usage();
        let observed = range.next(&mut work)?.ok_or(IndexError::ResourceLimited)?;
        check_eq!(observed.term.as_slice(), start);
        check_eq!(observed.owner, [expected_owner; 32]);
        check_eq!(observed.next, Some(object(expected_owner)));
        let after = work.usage();
        Ok::<_, IndexError>((
            after
                .reads
                .checked_sub(before.reads)
                .ok_or(IndexError::ResourceLimited)?,
            after
                .bytes
                .checked_sub(before.bytes)
                .ok_or(IndexError::ResourceLimited)?,
        ))
    };
    // Both starts are in the same packet and return one equally wide row.
    // A seek must not own or copy the 59 preceding term/owner rows.
    let first_usage = verify_one(first, 0)?;
    let last_usage = verify_one(last, 59)?;
    check_eq!(last_usage, first_usage);
    let mut retry = PackedRangeCursor::new(&spool, last, &mut work)?;
    let mut denied = WorkBudget::new(
        WorkLimits {
            reads: 1,
            writes: 0,
            bytes: 1_000_000,
            mutations: 0,
        },
        Instant::now()
            .checked_add(Duration::from_secs(10))
            .ok_or(IndexError::ResourceLimited)?,
    );
    check!(matches!(
        retry.next(&mut denied),
        Err(IndexError::ResourceLimited)
    ));
    let selected = retry.next(&mut work)?.ok_or(IndexError::ResourceLimited)?;
    check_eq!(selected.term.as_slice(), last);
    check!(retry.next(&mut work)?.is_none());
    Ok(())
}

#[test]
fn packed_range_copies_only_packet_boundary_and_retries_refusal() -> Result<(), IndexError> {
    let mut work = budget()?;
    let spool = TextSpool::new(&TestScratch, &mut work)?;
    spool.posting_batch(
        &[
            edit(b"\0first", 1, None, Some(object(10))),
            edit(b"\0middle", 2, Some(object(20)), Some(object(21))),
            edit(b"\0third", 3, Some(object(30)), None),
        ],
        &mut work,
    )?;
    let mut cursor = PackedRangeCursor::new(&spool, b"\0first", &mut work)?;
    let first = cursor.next(&mut work)?.ok_or(IndexError::ResourceLimited)?;
    check_eq!(first.term.as_slice(), b"\0first");

    // The authenticated packet already proves the order of these resident
    // rows. Returning them only copies the two caller-owned term keys.
    let mut exact = WorkBudget::new(
        WorkLimits {
            reads: 0,
            writes: 0,
            bytes: 13,
            mutations: 0,
        },
        Instant::now()
            .checked_add(Duration::from_secs(10))
            .ok_or(IndexError::ResourceLimited)?,
    );
    let middle = cursor
        .next(&mut exact)
        .map_err(|error| {
            IndexError::BackendViolation(format!(
                "resident middle row was refused: {error}; used {:?}",
                exact.usage()
            ))
        })?
        .ok_or(IndexError::ResourceLimited)?;
    check_eq!(middle.term.as_slice(), b"\0middle");
    check_eq!(middle.owner, [2; 32]);
    check_eq!(middle.expected, Some(object(20)));
    check_eq!(middle.next, Some(object(21)));
    let third = cursor
        .next(&mut exact)?
        .ok_or(IndexError::ResourceLimited)?;
    check_eq!(third.term.as_slice(), b"\0third");
    check_eq!(third.expected, Some(object(30)));
    check_eq!(third.next, None);
    check_eq!(
        exact.usage(),
        WorkUsage {
            bytes: 13,
            ..WorkUsage::default()
        }
    );
    // The next call crosses the page boundary. Refusing the boundary
    // witness must leave the cursor in a state that can retry to EOF.
    check!(matches!(
        cursor.next(&mut exact),
        Err(IndexError::ResourceLimited)
    ));
    check!(cursor.next(&mut work)?.is_none());
    let stable = work.usage();
    check!(cursor.next(&mut work)?.is_none());
    check_eq!(work.usage(), stable);
    Ok(())
}

#[test]
fn packed_range_checks_owner_order_between_authenticated_pages() -> Result<(), IndexError> {
    for next_owner in [6_u8, 7, 8] {
        let mut work = budget()?;
        let spool = TextSpool::new(&TestScratch, &mut work)?;
        let second = TextSpool::new(&TestScratch, &mut work)?;
        spool.posting_batch(
            &[
                edit(b"\0a", 1, None, Some(object(1))),
                edit(b"\0c", 7, None, Some(object(7))),
            ],
            &mut work,
        )?;
        second.posting_batch(
            &[
                edit(b"\0c", next_owner, None, Some(object(next_owner))),
                edit(b"\0d", 9, None, Some(object(9))),
            ],
            &mut work,
        )?;
        let _first_terms = spool.posting_term_page(None, &mut work)?;
        let _second_terms = second.posting_term_page(None, &mut work)?;

        // The original writer made each packet independently. Join the
        // second payload and its exact SQL fences without a global merger.
        let second_db = second.db.scoped(&work)?;
        let payload: Vec<u8> = second_db
            .query_row("SELECT payload FROM posting_packet", [], |row| row.get(0))
            .map_err(|error| sql(&error))?;
        drop(second_db);
        let db = spool.db.scoped(&work)?;
        let inserted = db
            .execute(
                "INSERT INTO posting_packet VALUES(0,0,1,x'0063',?1,x'0064',?2,?3)",
                params![[next_owner; 32].as_slice(), [9_u8; 32].as_slice(), payload],
            )
            .map_err(|error| sql(&error))?;
        check_eq!(inserted, 1);
        drop(db);

        let mut reader = PackedRangeCursor::new(&spool, b"\0a", &mut work)?;
        for (term, owner) in [(b"\0a", 1), (b"\0c", 7)] {
            let row = reader.next(&mut work)?.ok_or(IndexError::Conflict)?;
            check_eq!(row.term.as_slice(), term);
            check_eq!(row.owner, [owner; 32]);
        }
        if next_owner < 8 {
            for _ in 0..2 {
                check!(matches!(
                    reader.next(&mut work),
                    Err(IndexError::CorruptObject(message))
                        if message == "posting scratch packet: final packet stream is unordered"
                ));
            }
        } else {
            for (term, owner) in [(b"\0c", 8), (b"\0d", 9)] {
                let row = reader.next(&mut work)?.ok_or(IndexError::Conflict)?;
                check_eq!(row.term.as_slice(), term);
                check_eq!(row.owner, [owner; 32]);
            }
            check!(reader.next(&mut work)?.is_none());
        }
    }
    Ok(())
}

#[test]
fn packed_posting_duplicate_across_batches_refuses_all_later_reads() -> Result<(), IndexError> {
    let mut work = budget()?;
    let spool = TextSpool::new(&TestScratch, &mut work)?;
    let row = edit(b"\x00duplicate", 1, None, Some(object(1)));
    spool.posting_batch(std::slice::from_ref(&row), &mut work)?;
    spool.posting_batch(std::slice::from_ref(&row), &mut work)?;
    check!(matches!(
        spool.posting_term_page(None, &mut work),
        Err(IndexError::CorruptObject(_))
    ));
    check!(matches!(
        spool.posting_batch(std::slice::from_ref(&row), &mut work),
        Err(IndexError::BackendViolation(_))
    ));
    check!(matches!(
        spool.posting_root_cursors(&TermPage::new(), &[], &mut work),
        Err(IndexError::BackendViolation(_))
    ));
    Ok(())
}

#[test]
fn packed_posting_payload_and_fence_tamper_refuse_before_first_edit() -> Result<(), IndexError> {
    for fault in ["payload", "fence", "term"] {
        let mut work = budget()?;
        let spool = TextSpool::new(&TestScratch, &mut work)?;
        spool.posting_batch(&[edit(b"\x00anchor", 1, None, Some(object(1)))], &mut work)?;
        let terms = spool.posting_term_page(None, &mut work)?;
        let db = spool.db.scoped(&work)?;
        match fault {
            "payload" => {
                let _changed = db.execute("UPDATE posting_packet SET payload=zeroblob(length(payload)) WHERE phase=0 AND run=0 AND page=0",[]).map_err(|e|sql(&e))?;
            }
            "fence" => {
                let _changed = db.execute("UPDATE posting_packet SET first_term=x'0066' WHERE phase=0 AND run=0 AND page=0",[]).map_err(|e|sql(&e))?;
            }
            "term" => {
                let _changed = db.execute(
                    "UPDATE posting_term_packet SET payload=zeroblob(length(payload)) WHERE seq=0",
                    [],
                )
                .map_err(|e| sql(&e))?;
            }
            _ => unreachable!(),
        }
        drop(db);
        if fault == "term" {
            check!(matches!(
                spool.posting_term_page(None, &mut work),
                Err(IndexError::CorruptObject(_))
            ));
        } else {
            let mut cursor = spool.posting_cursor(terms);
            check!(matches!(
                cursor.next_edit(&mut work),
                Err(IndexError::CorruptObject(_))
            ));
        }
    }
    Ok(())
}

#[derive(Clone)]
struct RejectRetained;
impl MemoryScopePort for RejectRetained {
    fn reserve(&self, bytes: u64, lifetime: MemoryLifetime) -> Result<MemoryLease, IndexError> {
        if lifetime == MemoryLifetime::Retained {
            return Err(IndexError::ResourceLimited);
        }
        Ok(MemoryLease::new(bytes, Arc::new(self.clone())))
    }
}
impl MemoryLeaseOwner for RejectRetained {
    fn release(&self, _bytes: u64) {}
}

#[test]
fn packed_posting_retries_retained_edit_without_skipping_owner() -> Result<(), IndexError> {
    let mut work = budget()?;
    let spool = TextSpool::new(&TestScratch, &mut work)?;
    spool.posting_batch(
        &[
            edit(b"\0a", 1, None, Some(object(1))),
            edit(b"\0a", 2, None, Some(object(2))),
            edit(b"\0a", 3, None, Some(object(3))),
        ],
        &mut work,
    )?;
    let terms = spool.posting_term_page(None, &mut work)?;
    let mut cursor = spool.posting_cursor(terms);
    let first = cursor.next_edit(&mut work)?.ok_or(IndexError::Conflict)?;
    check_eq!(first.key(), &[1; 32]);

    let mut denied = budget()?;
    denied.bind_memory_scope(Some(MemoryScope::new(Arc::new(RejectRetained))));
    check!(matches!(
        cursor.next_edit(&mut denied),
        Err(IndexError::ResourceLimited)
    ));
    check_eq!(denied.usage().bytes, 0);

    let before = work.usage();
    for owner in [2, 3] {
        let row = cursor.next_edit(&mut work)?.ok_or(IndexError::Conflict)?;
        check_eq!(row.root(), 0);
        check_eq!(row.key(), &[owner; 32]);
        check_eq!(row.next()?, Some(object(owner)));
    }
    check_eq!(work.usage(), before);
    check!(cursor.next_edit(&mut work)?.is_none());
    Ok(())
}

#[test]
fn packed_posting_cancel_and_unfunded_memory_refuse_before_effect() -> Result<(), IndexError> {
    let mut setup = budget()?;
    let spool = TextSpool::new(&TestScratch, &mut setup)?;
    let row = edit(b"\x00funded", 1, None, Some(object(2)));
    let mut cancelled = budget()?;
    cancelled.request().cancel_handle().cancel();
    check!(matches!(
        spool.posting_batch(std::slice::from_ref(&row), &mut cancelled),
        Err(IndexError::Cancelled)
    ));
    spool.check_posting_usable()?;
    let mut denied = budget()?;
    denied.bind_memory_scope(Some(MemoryScope::new(Arc::new(RejectRetained))));
    check!(matches!(
        spool.posting_batch(std::slice::from_ref(&row), &mut denied),
        Err(IndexError::ResourceLimited)
    ));
    spool.check_posting_usable()?;
    spool.posting_batch(std::slice::from_ref(&row), &mut setup)?;
    let terms = spool.posting_term_page(None, &mut setup)?;
    check_eq!(terms.len(), 1);
    Ok(())
}
#[test]
fn failed_posting_seal_blocks_real_inventory_confirmation_custody() -> Result<(), IndexError> {
    use quanta_index_core::index::{
        AttemptId, CollectionId, OperationKey, ProducerId, SlotId, StreamId, TenantId,
        WorkspaceKey, WorktreeId,
    };
    let mut work = budget()?;
    let spool = TextSpool::new(&TestScratch, &mut work)?;
    let same = edit(b"\0duplicate", 1, None, Some(object(5)));
    spool.posting_batch(std::slice::from_ref(&same), &mut work)?;
    spool.posting_batch(std::slice::from_ref(&same), &mut work)?;
    check!(matches!(
        spool.posting_term_page(None, &mut work),
        Err(IndexError::CorruptObject(_))
    ));
    let reference = ObjectRef(Digest([91; 32]));
    let pending = spool.inventory_batch(
        &[(
            InventoryPartition::Planned(0),
            MapEdit {
                key: b"\0receipt".to_vec(),
                expected: None,
                next: Some(MapValue::Object(reference)),
            },
        )],
        &mut work,
    );
    check!(matches!(pending, Err(IndexError::BackendViolation(_))));
    let attempt = AttemptRef {
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
    };
    check!(matches!(
        spool.admit_inventory_confirmation(&[reference], 1, attempt, true, &mut work),
        Err(IndexError::BackendViolation(_))
    ));
    check!(matches!(
        spool.require_ready_packet(),
        Err(IndexError::BackendViolation(_))
    ));
    Ok(())
}

#[test]
fn packed_posting_two_logical_edits_refuse_one_write_before_pending_effect()
-> Result<(), IndexError> {
    let mut setup = budget()?;
    let spool = TextSpool::new(&TestScratch, &mut setup)?;
    let rows = [
        edit(b"\0a", 1, None, Some(object(1))),
        edit(b"\0b", 2, None, Some(object(2))),
    ];
    let mut underfunded = WorkBudget::new(
        WorkLimits {
            reads: 1_000,
            writes: 1,
            bytes: 10_000_000,
            mutations: 1_000,
        },
        Instant::now()
            .checked_add(Duration::from_secs(10))
            .ok_or(IndexError::ResourceLimited)?,
    );
    check!(matches!(
        spool.posting_batch(&rows, &mut underfunded),
        Err(IndexError::ResourceLimited)
    ));
    check_eq!(underfunded.usage().writes, 0);
    spool.posting_batch(&rows, &mut setup)?;
    let terms = spool.posting_term_page(None, &mut setup)?;
    check_eq!(terms.len(), 2);
    Ok(())
}
#[test]
fn packed_posting_write_rejects_poisoned_inventory_before_pending_effect() -> Result<(), IndexError>
{
    let mut work = budget()?;
    let spool = TextSpool::new(&TestScratch, &mut work)?;
    spool.invalidate_inventory();
    check!(matches!(
        spool.posting_batch(&[edit(b"\0a", 1, None, Some(object(1)))], &mut work),
        Err(IndexError::BackendViolation(_))
    ));
    Ok(())
}

#[test]
fn packed_cached_case_page_rejects_poisoned_inventory_before_next_page() -> Result<(), IndexError> {
    let mut work = budget()?;
    let spool = TextSpool::new(&TestScratch, &mut work)?;
    let mut rows = Vec::new();
    for index in 0_u16..63 {
        rows.push(edit(
            format!("\0a{index:03}").as_bytes(),
            1,
            None,
            Some(object(2)),
        ));
    }
    rows.push(edit(b"\0m", 1, None, Some(object(2))));
    rows.push(edit(b"\x01m", 2, None, Some(object(2))));
    for chunk in rows.chunks(quanta_index_scratch_sqlite::MAX_WRITE_BATCH_ROWS) {
        spool.posting_batch(chunk, &mut work)?;
    }
    let mut pages = PostingCasePages::new();
    let first = pages.next(&spool, &mut work)?;
    check_eq!(first.len(), 63);
    spool.invalidate_inventory();
    check!(matches!(
        pages.next(&spool, &mut work),
        Err(IndexError::BackendViolation(_))
    ));
    Ok(())
}
#[test]
fn packed_posting_seal_preinsert_write_refusal_preserves_funded_retry() -> Result<(), IndexError> {
    let mut setup = budget()?;
    let spool = TextSpool::new(&TestScratch, &mut setup)?;
    spool.posting_batch(&[edit(b"\0a", 1, None, Some(object(1)))], &mut setup)?;
    let mut denied = WorkBudget::new(
        WorkLimits {
            reads: 1000,
            writes: 0,
            bytes: 10_000_000,
            mutations: 1000,
        },
        Instant::now()
            .checked_add(Duration::from_secs(10))
            .ok_or(IndexError::ResourceLimited)?,
    );
    check!(matches!(
        spool.posting_term_page(None, &mut denied),
        Err(IndexError::ResourceLimited)
    ));
    check_eq!(denied.usage().writes, 0);
    let page = spool.posting_term_page(None, &mut setup)?;
    check_eq!(page.len(), 1);
    check_eq!(
        page.get(0).ok_or(IndexError::ResourceLimited)?.as_slice(),
        b"\0a"
    );
    Ok(())
}

/// A page-scoped reader may retain a checked packet, but a raw same-connection
/// mutation between two roots must invalidate that witness before the next edit.
#[test]
fn packed_posting_second_root_rejects_payload_tamper_after_first_root_cached()
-> Result<(), IndexError> {
    let mut work = budget()?;
    let spool = TextSpool::new(&TestScratch, &mut work)?;
    let mut rows = Vec::new();
    for term in [b"\0alpha".as_slice(), b"\0beta".as_slice()] {
        for owner in 0_u8..9 {
            rows.push(edit(term, owner, None, Some(object(1))));
        }
    }
    spool.posting_batch(&rows, &mut work)?;
    let terms = spool.posting_term_page(None, &mut work)?;
    check_eq!(terms.len(), 2);
    let mut roots = spool.posting_root_cursors(&terms, &[false, false], &mut work)?;
    {
        let mut first = roots.cursor(0, b"\0alpha")?;
        for owner in 0_u8..9 {
            let actual = first
                .next_edit(&mut work)?
                .ok_or(IndexError::ResourceLimited)?;
            let expected_owner = vec![owner; 32];
            check_eq!(actual.key(), expected_owner);
        }
        check!(first.next_edit(&mut work)?.is_none());
    }
    let db = spool.db.scoped(&work)?;
    let _changed = db
        .execute(
            "UPDATE posting_packet SET payload=zeroblob(length(payload)) WHERE run=0 AND page=0",
            [],
        )
        .map_err(|error| sql(&error))?;
    drop(db);
    let mut second = roots.cursor(1, b"\0beta")?;
    check!(matches!(
        second.next_edit(&mut work),
        Err(IndexError::CorruptObject(_) | IndexError::BackendViolation(_))
    ));
    Ok(())
}

#[test]
fn packed_prefetched_posting_rejects_scratch_change_before_first_edit() -> Result<(), IndexError> {
    let mut work = budget()?;
    let spool = TextSpool::new(&TestScratch, &mut work)?;
    spool.posting_batch(
        &[
            edit(b"\0alpha", 1, None, Some(object(1))),
            edit(b"\0beta", 2, None, Some(object(1))),
            edit(b"\0gamma", 3, None, Some(object(1))),
        ],
        &mut work,
    )?;
    let terms = spool.posting_term_page(None, &mut work)?;
    let mut roots = spool.posting_root_cursors(&terms, &[true, true, false], &mut work)?;
    let mut sparse = roots.cursor(0, b"\0alpha")?;
    let db = spool.db.scoped(&work)?;
    let changed = db
        .execute(
            "UPDATE posting_packet SET payload=payload WHERE run=0 AND page=0",
            [],
        )
        .map_err(|error| sql(&error))?;
    check!(changed > 0);
    drop(db);
    check!(matches!(
        sparse.next_edit(&mut work),
        Err(IndexError::BackendViolation(_))
    ));
    check!(matches!(
        roots.cursor(2, b"\0gamma"),
        Err(IndexError::BackendViolation(_))
    ));
    Ok(())
}

#[test]
fn packed_posting_all_unique_objects_and_inline_noop_preserve_original_rows()
-> Result<(), IndexError> {
    let mut work = budget()?;
    let spool = TextSpool::new(&TestScratch, &mut work)?;
    let inline = MapValue::inline(b"tiny", [ObjectRef(Digest([0x4a; 32]))])?;
    let mut rows = Vec::new();
    for owner in 0_u8..128 {
        rows.push(edit(
            b"\0allunique",
            owner,
            None,
            Some(MapValue::Object(ObjectRef(Digest([owner; 32])))),
        ));
    }
    spool.posting_batch(&rows, &mut work)?;
    spool.posting_batch(
        &[edit(b"\0allunique", 250, Some(inline), Some(inline))],
        &mut work,
    )?;
    let terms = spool.posting_term_page(None, &mut work)?;
    check_eq!(terms.len(), 1);
    let mut cursor = spool.posting_cursor(terms);
    for owner in 0_u8..128 {
        let actual = cursor
            .next_edit(&mut work)?
            .ok_or(IndexError::ResourceLimited)?;
        let expected_owner = vec![owner; 32];
        check_eq!(actual.key(), expected_owner);
        check_eq!(actual.expected()?, None);
        check_eq!(
            actual.next()?,
            Some(MapValue::Object(ObjectRef(Digest([owner; 32]))))
        );
    }
    let noop = cursor
        .next_edit(&mut work)?
        .ok_or(IndexError::ResourceLimited)?;
    let noop_owner = vec![250; 32];
    check_eq!(noop.key(), noop_owner.as_slice());
    check_eq!(noop.expected()?, Some(inline));
    check_eq!(noop.next()?, Some(inline));
    check!(cursor.next_edit(&mut work)?.is_none());
    Ok(())
}

#[test]
fn packed_posting_invalid_dictionary_id_refuses_even_with_recomputed_packet_digest()
-> Result<(), IndexError> {
    use sha2::{Digest as _, Sha256};
    let mut work = budget()?;
    let spool = TextSpool::new(&TestScratch, &mut work)?;
    spool.posting_batch(&[edit(b"\0anchor", 1, None, Some(object(1)))], &mut work)?;
    let terms = spool.posting_term_page(None, &mut work)?;
    let db = spool.db.scoped(&work)?;
    let mut raw: Vec<u8> = db
        .query_row(
            "SELECT payload FROM posting_packet WHERE run=0 AND page=0 LIMIT 1",
            [],
            |row| row.get(0),
        )
        .map_err(|error| sql(&error))?;
    // Private v2 packet: 50-byte header, 2-byte term width, then owner ID.
    // Corruption is independently checked after recomputing the packet hash.
    let owner_id = 50 + 2 + b"\0anchor".len();
    raw.get_mut(owner_id..owner_id + 2)
        .ok_or(IndexError::ResourceLimited)?
        .copy_from_slice(&u16::MAX.to_be_bytes());
    let digest = Sha256::digest(raw.get(42..).ok_or(IndexError::ResourceLimited)?);
    raw.get_mut(10..42)
        .ok_or(IndexError::ResourceLimited)?
        .copy_from_slice(&digest);
    let _changed = db
        .execute(
            "UPDATE posting_packet SET payload=?1 WHERE run=0 AND page=0",
            [&raw],
        )
        .map_err(|error| sql(&error))?;
    drop(db);
    let mut cursor = spool.posting_cursor(terms);
    check!(matches!(
        cursor.next_edit(&mut work),
        Err(IndexError::CorruptObject(_))
    ));
    Ok(())
}

#[test]
fn packed_root_partial_probe_replays_same_slot_and_skips_only_forward() -> Result<(), IndexError> {
    let mut work = budget()?;
    let spool = TextSpool::new(&TestScratch, &mut work)?;
    let mut rows = Vec::new();
    for term in [
        b"\0alpha".as_slice(),
        b"\0beta".as_slice(),
        b"\0gamma".as_slice(),
    ] {
        for owner in 0_u8..9 {
            rows.push(edit(term, owner, None, Some(object(1))));
        }
    }
    spool.posting_batch(&rows, &mut work)?;
    let terms = spool.posting_term_page(None, &mut work)?;
    let mut roots = spool.posting_root_cursors(&terms, &[false, false, false], &mut work)?;
    {
        let mut partial = roots.cursor(0, b"\0alpha")?;
        let first = partial
            .next_edit(&mut work)?
            .ok_or(IndexError::ResourceLimited)?;
        let first_owner = vec![0; 32];
        check_eq!(first.key(), first_owner);
    }
    check!(matches!(
        roots.cursor(0, b"\0altered"),
        Err(IndexError::CorruptObject(_))
    ));
    {
        let mut replay = roots.cursor(0, b"\0alpha")?;
        for owner in 0_u8..9 {
            let actual = replay
                .next_edit(&mut work)?
                .ok_or(IndexError::ResourceLimited)?;
            let expected_owner = vec![owner; 32];
            check_eq!(actual.key(), expected_owner);
        }
        check!(replay.next_edit(&mut work)?.is_none());
    }
    {
        let mut forward = roots.cursor(2, b"\0gamma")?;
        for owner in 0_u8..9 {
            let actual = forward
                .next_edit(&mut work)?
                .ok_or(IndexError::ResourceLimited)?;
            let expected_owner = vec![owner; 32];
            check_eq!(actual.key(), expected_owner);
        }
        check!(forward.next_edit(&mut work)?.is_none());
    }
    check!(matches!(
        roots.cursor(0, b"\0alpha"),
        Err(IndexError::CorruptObject(_))
    ));
    Ok(())
}

#[test]
fn packed_posting_two_unique_values_per_row_seal_and_readback_match_fixed_gold()
-> Result<(), IndexError> {
    fn reference(number: u32) -> ObjectRef {
        let mut bytes = [0_u8; 32];
        bytes[..4].copy_from_slice(&number.to_be_bytes());
        ObjectRef(Digest(bytes))
    }
    let mut work = budget()?;
    let spool = TextSpool::new(&TestScratch, &mut work)?;
    let mut batch = Vec::new();
    for index in 0_u32..900 {
        let mut owner = [0_u8; 32];
        owner[..4].copy_from_slice(&index.to_be_bytes());
        batch.push((
            b"\0unique".to_vec(),
            MapEdit {
                key: owner.to_vec(),
                expected: Some(MapValue::Object(reference(index * 2))),
                next: Some(MapValue::Object(reference(index * 2 + 1))),
            },
        ));
        if batch.len() == quanta_index_scratch_sqlite::MAX_WRITE_BATCH_ROWS {
            spool.posting_batch(&batch, &mut work)?;
            batch.clear();
        }
    }
    if !batch.is_empty() {
        spool.posting_batch(&batch, &mut work)?;
    }
    let terms = spool.posting_term_page(None, &mut work)?;
    check_eq!(terms.len(), 1);
    check_eq!(
        terms.get(0).ok_or(IndexError::ResourceLimited)?.as_slice(),
        b"\0unique"
    );
    let mut cursor = spool.posting_cursor(terms);
    for index in 0_u32..900 {
        let actual = cursor
            .next_edit(&mut work)?
            .ok_or(IndexError::ResourceLimited)?;
        let mut owner = [0_u8; 32];
        owner[..4].copy_from_slice(&index.to_be_bytes());
        check_eq!(actual.root(), 0);
        let expected_owner = owner.to_vec();
        check_eq!(actual.key(), expected_owner);
        check_eq!(
            actual.expected()?,
            Some(MapValue::Object(reference(index * 2)))
        );
        check_eq!(
            actual.next()?,
            Some(MapValue::Object(reference(index * 2 + 1)))
        );
    }
    check!(cursor.next_edit(&mut work)?.is_none());
    Ok(())
}

#[test]
fn paired_case_root_seek_skips_unselected_sensitive_gap_under_original_read_budget()
-> Result<(), IndexError> {
    let mut setup = WorkBudget::new(
        WorkLimits {
            reads: 1_000_000,
            writes: 1_000_000,
            bytes: 536_870_912,
            mutations: 1_000_000,
        },
        Instant::now()
            .checked_add(Duration::from_secs(120))
            .ok_or(IndexError::ResourceLimited)?,
    );
    let spool = TextSpool::new(&TestScratch, &mut setup)?;
    let value = object(0x53);
    let mut batch = Vec::new();
    for term in std::iter::once(b"\0alpha".to_vec())
        .chain((0..5_000).map(|index| format!("\0z{index:05}").into_bytes()))
        .chain(std::iter::once(b"\x01alpha".to_vec()))
    {
        for owner in 0_u8..9 {
            batch.push(edit(&term, owner, None, Some(value)));
            if batch.len() == quanta_index_scratch_sqlite::MAX_WRITE_BATCH_ROWS {
                spool.posting_batch(&batch, &mut setup)?;
                batch.clear();
            }
        }
    }
    if !batch.is_empty() {
        spool.posting_batch(&batch, &mut setup)?;
    }
    let mut paired = PostingCasePages::new();
    let page = paired.next(&spool, &mut setup)?;
    check_eq!(page.len(), 64);
    check_eq!(
        page.get(0).ok_or(IndexError::ResourceLimited)?.as_slice(),
        b"\0alpha"
    );
    check_eq!(
        page.get(63).ok_or(IndexError::ResourceLimited)?.as_slice(),
        b"\x01alpha"
    );
    let mut scan = WorkBudget::new(
        WorkLimits {
            reads: 1_000,
            writes: 0,
            bytes: 1_048_576,
            mutations: 0,
        },
        Instant::now()
            .checked_add(Duration::from_secs(30))
            .ok_or(IndexError::ResourceLimited)?,
    );
    let eligible = vec![false; page.len()];
    let mut roots = spool.posting_root_cursors(&page, &eligible, &mut scan)?;
    for (slot, term) in [(0, b"\0alpha".as_slice()), (63, b"\x01alpha".as_slice())] {
        let mut cursor = roots.cursor(slot, term)?;
        for owner in 0_u8..9 {
            let actual = cursor
                .next_edit(&mut scan)?
                .ok_or(IndexError::ResourceLimited)?;
            check_eq!(actual.root(), 0);
            check_eq!(actual.key(), vec![owner; 32]);
            check_eq!(actual.expected()?, None);
            check_eq!(actual.next()?, Some(value));
        }
        check!(cursor.next_edit(&mut scan)?.is_none());
    }
    drop(roots);
    let mut global = spool.posting_cursor(page);
    for slot in 0_u32..64 {
        for owner in 0_u8..9 {
            let actual = global
                .next_edit(&mut setup)?
                .ok_or(IndexError::ResourceLimited)?;
            check_eq!(actual.root(), slot);
            check_eq!(actual.key(), vec![owner; 32]);
            check_eq!(actual.next()?, Some(value));
        }
    }
    check!(global.next_edit(&mut setup)?.is_none());
    Ok(())
}
