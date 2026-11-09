use super::*;

#[derive(serde::Serialize)]
struct FixedOccurrence {
    positions: [u32; 1],
}

fn push_fixed(
    writes: &mut OccurrenceWrites,
    ordinal: u32,
    key: &[u8],
    identified: ObjectRef,
    position: u32,
    spool: &TextSpool,
    edits: &mut InventoryEdits,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    let payload = super::super::super::text_memory::encode(
        &FixedOccurrence {
            positions: [position],
        },
        budget,
    )?;
    writes.push(
        OccurrenceEdit {
            ordinal,
            key: PreparedKey::copy(key, budget)?,
            prior: None,
            identified,
        },
        payload,
        spool,
        edits,
        budget,
    )
}

#[test]
fn packed_alias_accepts_cold_ordinals_beyond_sql_page_and_refuses_window_end()
-> Result<(), IndexError> {
    let mut fixture = visitor_fixture()?;
    fixture.objects.install_outputs = Some(vec![reference(90), reference(91)]);
    let scratch = TestScratch::default();
    let custody = TestCustody;
    let mut work = budget()?;
    let spool = TextSpool::new(&scratch, &mut work)?;
    let mut writes = OccurrenceWrites::new();
    let mut edits = InventoryEdits::new(0)?;
    let mut last_slot = None;
    for (ordinal, identified, position) in [(64, reference(90), 0), (127, reference(91), 1)] {
        let payload = super::super::super::text_memory::encode(
            &FixedOccurrence {
                positions: [position],
            },
            &work,
        )?;
        let slot = writes.push_with_slot(
            OccurrenceEdit {
                ordinal,
                key: PreparedKey::copy(b"\0a", &work)?,
                prior: None,
                identified,
            },
            payload,
            &spool,
            &mut edits,
            &mut work,
        )?;
        last_slot = Some(slot);
        writes.push_block_alias(
            ordinal,
            PreparedKey::copy(b"\0b", &work)?,
            slot,
            &spool,
            &mut edits,
            &mut work,
        )?;
    }
    let last_slot = last_slot.ok_or(IndexError::ResourceLimited)?;
    check!(matches!(
        writes.push_block_alias(
            256,
            PreparedKey::copy(b"\0c", &work)?,
            last_slot,
            &spool,
            &mut edits,
            &mut work,
        ),
        Err(IndexError::BackendViolation(_))
    ));
    let rows_before = edits.rows.len();
    check!(matches!(
        writes.push_block_alias(
            127,
            PreparedKey::copy(b"\0c", &work)?,
            PendingOccurrenceSlot {
                index: 0,
                identified: reference(91),
            },
            &spool,
            &mut edits,
            &mut work,
        ),
        Err(IndexError::BackendViolation(_))
    ));
    check_eq!(edits.rows.len(), rows_before);
    check_eq!(writes.pending.len(), 2);
    check_eq!(writes.fanout_rows, 4);
    let mut confirmed = ConfirmedOccurrences::new(0)?;
    let mut context = AttemptContext {
        attempt: test_attempt(fixture.ticket.view.identity.workspace),
        objects: &fixture.objects,
        maps: &fixture.maps,
        custody: &custody,
        scratch: &scratch,
        budget: &mut work,
    };
    writes.flush(&mut context, &spool, &mut confirmed, &mut edits)?;
    check_eq!(fixture.objects.install_batches.load(Ordering::Relaxed), 1);
    check_eq!(
        fixture.objects.install_batch_inputs.load(Ordering::Relaxed),
        2
    );
    edits.flush(&spool, context.budget)?;
    let mut cursor = spool.inventory_cursor(128)?;
    for (ordinal, identified) in [(64, reference(90)), (127, reference(91))] {
        for key in [b"\0a".as_slice(), b"\0b".as_slice()] {
            let edit = cursor
                .next_edit(context.budget)?
                .ok_or_else(|| IndexError::BackendViolation("packed alias edit missing".into()))?;
            check_eq!(edit.root, ordinal);
            check_eq!(edit.edit.key.as_slice(), key);
            check_eq!(edit.edit.next, Some(identified.into()));
        }
    }
    check!(cursor.next_edit(context.budget)?.is_none());
    Ok(())
}

#[test]
fn mixed_owner_packet_installs_once_and_keeps_each_inventory_ordinal() -> Result<(), IndexError> {
    let mut fixture = visitor_fixture()?;
    fixture.objects.install_outputs = Some(vec![reference(90), reference(91), reference(92)]);
    let scratch = TestScratch::default();
    let custody = TestCustody;
    let mut work = budget()?;
    let spool = TextSpool::new(&scratch, &mut work)?;
    let mut writes = OccurrenceWrites::new();
    let mut edits = InventoryEdits::new(1)?;
    push_fixed(
        &mut writes,
        0,
        b"\0a",
        reference(90),
        0,
        &spool,
        &mut edits,
        &mut work,
    )?;
    push_fixed(
        &mut writes,
        1,
        b"\0a",
        reference(91),
        1,
        &spool,
        &mut edits,
        &mut work,
    )?;
    push_fixed(
        &mut writes,
        1,
        b"\0b",
        reference(92),
        2,
        &spool,
        &mut edits,
        &mut work,
    )?;
    check_eq!(fixture.objects.install_batches.load(Ordering::Relaxed), 0);
    let mut confirmed = ConfirmedOccurrences::new(0)?;
    let mut context = AttemptContext {
        attempt: test_attempt(fixture.ticket.view.identity.workspace),
        objects: &fixture.objects,
        maps: &fixture.maps,
        custody: &custody,
        scratch: &scratch,
        budget: &mut work,
    };
    writes.flush(&mut context, &spool, &mut confirmed, &mut edits)?;
    check_eq!(fixture.objects.install_batches.load(Ordering::Relaxed), 1);
    check_eq!(
        fixture.objects.install_batch_inputs.load(Ordering::Relaxed),
        3
    );
    edits.flush(&spool, context.budget)?;
    let mut cursor = spool.inventory_cursor(2)?;
    for (ordinal, key, identified) in [
        (0, b"\0a".as_slice(), reference(90)),
        (1, b"\0a".as_slice(), reference(91)),
        (1, b"\0b".as_slice(), reference(92)),
    ] {
        let row = cursor
            .next_edit(context.budget)?
            .ok_or_else(|| IndexError::BackendViolation("missing mixed-owner edit".into()))?;
        check_eq!(row.root, ordinal);
        check_eq!(row.edit.key.as_slice(), key);
        check_eq!(row.edit.expected, None);
        check_eq!(row.edit.next, Some(identified.into()));
    }
    check!(cursor.next_edit(context.budget)?.is_none());
    Ok(())
}

#[test]
fn same_leaf_across_owners_has_one_install_and_two_edits() -> Result<(), IndexError> {
    let mut fixture = visitor_fixture()?;
    fixture.objects.install_outputs = Some(vec![reference(90)]);
    let scratch = TestScratch::default();
    let custody = TestCustody;
    let mut work = budget()?;
    let spool = TextSpool::new(&scratch, &mut work)?;
    let mut writes = OccurrenceWrites::new();
    let mut edits = InventoryEdits::new(1)?;
    for ordinal in [0, 1] {
        push_fixed(
            &mut writes,
            ordinal,
            b"\0a",
            reference(90),
            0,
            &spool,
            &mut edits,
            &mut work,
        )?;
    }
    let mut confirmed = ConfirmedOccurrences::new(2)?;
    let mut context = AttemptContext {
        attempt: test_attempt(fixture.ticket.view.identity.workspace),
        objects: &fixture.objects,
        maps: &fixture.maps,
        custody: &custody,
        scratch: &scratch,
        budget: &mut work,
    };
    writes.flush(&mut context, &spool, &mut confirmed, &mut edits)?;
    check_eq!(fixture.objects.install_batches.load(Ordering::Relaxed), 1);
    check_eq!(
        fixture.objects.install_batch_inputs.load(Ordering::Relaxed),
        1
    );
    check_eq!(confirmed.references.len(), 1);
    edits.flush(&spool, context.budget)?;
    let mut cursor = spool.inventory_cursor(2)?;
    for ordinal in [0, 1] {
        let row = cursor
            .next_edit(context.budget)?
            .ok_or_else(|| IndexError::BackendViolation("missing duplicate-leaf edit".into()))?;
        check_eq!(row.root, ordinal);
        check_eq!(row.edit.key.as_slice(), b"\0a");
        check_eq!(row.edit.next, Some(reference(90).into()));
    }
    check!(cursor.next_edit(context.budget)?.is_none());
    Ok(())
}

#[test]
fn later_wrong_output_cannot_publish_a_confirmed_owner_prefix() -> Result<(), IndexError> {
    let mut fixture = visitor_fixture()?;
    fixture.objects.install_outputs = Some(vec![reference(90), reference(99)]);
    let scratch = TestScratch::default();
    let custody = TestCustody;
    let mut work = budget()?;
    let spool = TextSpool::new(&scratch, &mut work)?;
    let mut writes = OccurrenceWrites::new();
    let mut edits = InventoryEdits::new(1)?;
    push_fixed(
        &mut writes,
        0,
        b"\0a",
        reference(90),
        0,
        &spool,
        &mut edits,
        &mut work,
    )?;
    push_fixed(
        &mut writes,
        1,
        b"\0b",
        reference(91),
        1,
        &spool,
        &mut edits,
        &mut work,
    )?;
    let mut confirmed = ConfirmedOccurrences::new(2)?;
    let mut context = AttemptContext {
        attempt: test_attempt(fixture.ticket.view.identity.workspace),
        objects: &fixture.objects,
        maps: &fixture.maps,
        custody: &custody,
        scratch: &scratch,
        budget: &mut work,
    };
    check!(matches!(
        writes.flush(&mut context, &spool, &mut confirmed, &mut edits),
        Err(IndexError::BackendViolation(message))
            if message == "native text occurrence identity changed during installation"
    ));
    check_eq!(
        fixture.objects.install_batch_inputs.load(Ordering::Relaxed),
        2
    );
    check!(confirmed.references.is_empty());
    check_eq!(writes.pending.len(), 2);
    check!(
        spool
            .inventory_cursor(2)?
            .next_edit(context.budget)
            .is_err()
    );
    Ok(())
}

#[test]
fn cancelled_or_foreign_attempt_cannot_install_a_queued_owner() -> Result<(), IndexError> {
    for cancelled in [false, true] {
        let fixture = visitor_fixture()?;
        let scratch = TestScratch::default();
        let custody = TestCustody;
        let mut work = budget()?;
        let spool = TextSpool::new(&scratch, &mut work)?;
        let mut writes = OccurrenceWrites::new();
        let mut edits = InventoryEdits::new(0)?;
        push_fixed(
            &mut writes,
            0,
            b"\0a",
            reference(90),
            0,
            &spool,
            &mut edits,
            &mut work,
        )?;
        let attempt = test_attempt(fixture.ticket.view.identity.workspace);
        let mut confirmed = ConfirmedOccurrences::new(2)?;
        if cancelled {
            work.request().cancel_handle().cancel();
        } else {
            confirmed.prepare(attempt, &work)?;
        }
        let mut wrong = attempt;
        wrong.fence = wrong
            .fence
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        let mut context = AttemptContext {
            attempt: if cancelled { attempt } else { wrong },
            objects: &fixture.objects,
            maps: &fixture.maps,
            custody: &custody,
            scratch: &scratch,
            budget: &mut work,
        };
        let result = writes.flush(&mut context, &spool, &mut confirmed, &mut edits);
        if cancelled {
            check_eq!(result, Err(IndexError::Cancelled));
        } else {
            check_eq!(result, Err(IndexError::FenceLost));
        }
        check_eq!(fixture.objects.install_batches.load(Ordering::Relaxed), 0);
        check!(confirmed.references.is_empty());
        check_eq!(writes.pending.len(), 1);
        check!(
            spool
                .inventory_cursor(1)?
                .next_edit(&mut budget()?)?
                .is_none()
        );
    }
    Ok(())
}

#[test]
fn cross_owner_pending_payload_grants_remain_bounded_and_refund_on_drop() -> Result<(), IndexError>
{
    let (mut work, memory) = funded_token_budget(1_000_000)?;
    let scratch = TestScratch::default();
    let spool = TextSpool::new(&scratch, &mut work)?;
    let mut edits = InventoryEdits::new(0)?;
    let mut writes = OccurrenceWrites::new();
    check_eq!(memory.calls.load(Ordering::Relaxed), 0);
    for ordinal in 0..8_u32 {
        push_fixed(
            &mut writes,
            ordinal,
            b"\0a",
            reference(u8::try_from(ordinal).map_err(|_error| IndexError::ResourceLimited)?),
            ordinal,
            &spool,
            &mut edits,
            &mut work,
        )?;
    }
    check_eq!(writes.pending.len(), 8);
    check!(writes.inputs.capacity() <= 64);
    check!(memory.live.load(Ordering::Relaxed) > 0);
    drop(writes);
    drop(edits);
    drop(spool);
    check_eq!(memory.live.load(Ordering::Relaxed), 0);
    check!(!memory.invalid_release.load(Ordering::Relaxed));
    Ok(())
}

fn new_owner_change(
    fixture: &mut VisitorFixture,
    id: u8,
    body: &[u8],
) -> Result<SourceChange, IndexError> {
    let body_ref = reference(id);
    let record_ref = reference(id.checked_add(1).ok_or(IndexError::ResourceLimited)?);
    let mut record = fixture_record(fixture)?;
    record.path = format!("owner-{id}");
    record.bytes = body_ref;
    record.byte_len = u64::try_from(body.len()).map_err(|_error| IndexError::ResourceLimited)?;
    fixture.objects.add(body_ref, body.to_vec(), Vec::new())?;
    fixture
        .objects
        .add(record_ref, encode(&record)?, vec![body_ref])?;
    Ok(SourceChange {
        owner: OwnerKey {
            producer: fixture.owner.producer,
            stable_id: StableOwnerId([id; 16]),
        },
        prior: None,
        next: Some((record_ref, SourceRecordOwner::new_unscoped(record))),
    })
}

#[test]
fn inventory_window_defers_sparse_file_packets_and_refuses_before_any_root_apply()
-> Result<(), IndexError> {
    let mut fixture = visitor_fixture()?;
    let changes = [
        new_owner_change(&mut fixture, 140, b"alpha")?,
        new_owner_change(&mut fixture, 142, b"alpha beta")?,
        new_owner_change(&mut fixture, 144, b"alpha beta gamma")?,
    ];
    let scratch = TestScratch::default();
    let custody = TestCustody;
    let mut work = budget()?;
    let spool = TextSpool::new(&scratch, &mut work)?;
    let mut context = AttemptContext {
        attempt: test_attempt(fixture.ticket.view.identity.workspace),
        objects: &fixture.objects,
        maps: &fixture.maps,
        custody: &custody,
        scratch: &scratch,
        budget: &mut work,
    };
    let mut root = Root::default();
    let mut batch = InventoryBatch::new(64)?;
    let mut contributions = ContributionWrites::new()?;
    let mut inventories = InventoryReuse::new(fixture.recipe, 64)?;
    for change in &changes {
        batch.push_change(
            change,
            &mut root,
            &mut context,
            &spool,
            &mut contributions,
            &mut inventories,
        )?;
    }
    check_eq!(batch.rows.len(), 3);
    check_eq!(batch.bases.len(), 3);
    check_eq!(batch.occurrence_writes.pending.len(), 3);
    check_eq!(fixture.objects.install_batches.load(Ordering::Relaxed), 0);
    check_eq!(fixture.maps.map_applies.load(Ordering::Relaxed), 0);
    check_eq!(root.docs, 0);
    check!(matches!(
        batch.flush(
            &mut root,
            &mut context,
            &spool,
            &mut contributions,
            &mut inventories,
        ),
        Err(IndexError::BackendViolation(message))
            if message == "changed-body occurrence build reached object batch"
    ));
    // Position leaves [0], [1], [2] are fixed facts of the three bodies.
    // All twelve raw/folded edits share one physical three-input packet.
    check_eq!(fixture.objects.install_batches.load(Ordering::Relaxed), 1);
    check_eq!(
        fixture.objects.install_batch_inputs.load(Ordering::Relaxed),
        3
    );
    check_eq!(fixture.maps.map_applies.load(Ordering::Relaxed), 0);
    check!(inventories.occurrences.references.is_empty());
    check_eq!(root.owners, MapRoot(None));
    check_eq!(root.terms, MapRoot(None));
    check_eq!(root.docs, 0);
    check!(
        spool
            .inventory_cursor(3)?
            .next_edit(context.budget)
            .is_err()
    );
    Ok(())
}

#[test]
fn unchanged_owner_can_pop_its_base_without_stealing_a_pending_ordinal() -> Result<(), IndexError> {
    let mut fixture = visitor_fixture()?;
    let first = new_owner_change(&mut fixture, 140, b"alpha")?;
    let last = new_owner_change(&mut fixture, 142, b"beta gamma")?;
    let prior_record = fixture_record(&fixture)?;
    let mut next_record = prior_record.clone();
    next_record.bytes = reference(144);
    next_record.byte_len = 4;
    fixture
        .objects
        .add(reference(144), b"one ".to_vec(), Vec::new())?;
    fixture
        .objects
        .add(reference(145), encode(&next_record)?, vec![reference(144)])?;
    let position_payload = encode(&FixedOccurrence { positions: [0] })?;
    let position_ref = ObjectRef(Digest(Sha256::digest(&position_payload).into()));
    fixture
        .objects
        .add(position_ref, position_payload, Vec::new())?;
    for key in [b"\0one", b"\x01one"] {
        fixture
            .maps
            .add(fixture.inventory_root, key.to_vec(), position_ref)?;
    }
    let middle = SourceChange {
        owner: fixture.owner,
        prior: Some((
            fixture.source_ref,
            SourceRecordOwner::new_unscoped(prior_record),
        )),
        next: Some((reference(145), SourceRecordOwner::new_unscoped(next_record))),
    };
    let scratch = TestScratch::default();
    let custody = TestCustody;
    let mut work = budget()?;
    let spool = TextSpool::new(&scratch, &mut work)?;
    let mut context = AttemptContext {
        attempt: test_attempt(fixture.ticket.view.identity.workspace),
        objects: &fixture.objects,
        maps: &fixture.maps,
        custody: &custody,
        scratch: &scratch,
        budget: &mut work,
    };
    let mut root: Root = decode(
        &fixture
            .objects
            .stored
            .get(&fixture.member.artifact)
            .ok_or_else(|| IndexError::CorruptObject("fixture root missing".into()))?
            .payload,
    )?;
    let mut batch = InventoryBatch::new(64)?;
    let mut contributions = ContributionWrites::new()?;
    let mut inventories = InventoryReuse::new(fixture.recipe, 64)?;
    for change in [&first, &middle, &last] {
        batch.push_change(
            change,
            &mut root,
            &mut context,
            &spool,
            &mut contributions,
            &mut inventories,
        )?;
    }
    check_eq!(batch.rows.len(), 3);
    check_eq!(batch.bases.len(), 2);
    check_eq!(batch.counts.as_slice(), &[2, 4]);
    check!(matches!(
        batch.rows.as_slice().get(1).map(|row| &row.next),
        Some(PendingTextNext::Ready)
    ));
    let ordinals: Vec<_> = batch
        .occurrence_writes
        .pending
        .iter()
        .map(|row| row.ordinal)
        .collect();
    check_eq!(ordinals, vec![1, 1]);
    check_eq!(batch.occurrence_writes.fanout_rows, 6);
    check_eq!(fixture.objects.install_batches.load(Ordering::Relaxed), 0);
    Ok(())
}

#[test]
fn physical_65_packet_preserves_191_aliases_and_starts_next_packet_after_explicit_flush()
-> Result<(), IndexError> {
    let mut fixture = visitor_fixture()?;
    fixture.objects.install_outputs = Some((90_u8..155).map(reference).collect());
    let scratch = TestScratch::default();
    let custody = TestCustody;
    let memory = Arc::new(TokenMemory {
        ceiling: 1_000_000,
        ..TokenMemory::default()
    });
    let mut work = packet_budget()?;
    work.bind_memory_scope(Some(quanta_index_core::index::MemoryScope::new(Arc::new(
        BatchOutputMemoryScope(Arc::clone(&memory)),
    ))));
    let spool = TextSpool::new(&scratch, &mut work)?;
    let mut edits = InventoryEdits::new(1)?;
    let mut writes = OccurrenceWrites::new();
    for alias in 0_u16..128 {
        let slot = u8::try_from(alias % 2).map_err(|_error| IndexError::ResourceLimited)?;
        let key = format!("\0alias-{alias:03}");
        push_fixed(
            &mut writes,
            u32::from(slot),
            key.as_bytes(),
            reference(90_u8.checked_add(slot).ok_or(IndexError::ResourceLimited)?),
            u32::from(slot),
            &spool,
            &mut edits,
            &mut work,
        )?;
    }
    check_eq!(writes.pending.len(), 2);
    check_eq!(writes.fanout_rows, 128);
    check_eq!(fixture.objects.install_batches.load(Ordering::Relaxed), 0);
    for slot in 2_u8..64 {
        let key = format!("\0unique-{slot:02}");
        push_fixed(
            &mut writes,
            0,
            key.as_bytes(),
            reference(90_u8.checked_add(slot).ok_or(IndexError::ResourceLimited)?),
            u32::from(slot),
            &spool,
            &mut edits,
            &mut work,
        )?;
    }
    check_eq!(writes.pending.len(), 64);
    check_eq!(writes.fanout_rows, 190);
    check!(writes.inputs.capacity() <= 64);
    push_fixed(
        &mut writes,
        1,
        b"\0next",
        reference(154),
        64,
        &spool,
        &mut edits,
        &mut work,
    )?;
    check_eq!(writes.pending.len(), 65);
    check_eq!(writes.fanout_rows, 191);
    check!(spool.inventory_cursor(2)?.next_edit(&mut work).is_err());
    let attempt = test_attempt(fixture.ticket.view.identity.workspace);
    let mut confirmed = ConfirmedOccurrences::new(0)?;
    writes.flush(
        &mut AttemptContext {
            attempt,
            objects: &fixture.objects,
            maps: &fixture.maps,
            custody: &custody,
            scratch: &scratch,
            budget: &mut work,
        },
        &spool,
        &mut confirmed,
        &mut edits,
    )?;
    check_eq!(fixture.objects.install_batches.load(Ordering::Relaxed), 1);
    check_eq!(
        fixture.objects.install_batch_inputs.load(Ordering::Relaxed),
        65
    );
    let mut cursor = spool.inventory_cursor(2)?;
    let mut count = 0_u16;
    while let Some(row) = cursor.next_edit(&mut work)? {
        let key = std::str::from_utf8(&row.edit.key)
            .map_err(|_error| IndexError::CorruptObject("fixture key is not UTF-8".into()))?;
        let slot = if let Some(alias) = key.strip_prefix("\0alias-") {
            let alias = alias
                .parse::<u16>()
                .map_err(|_error| IndexError::CorruptObject("fixture alias invalid".into()))?;
            let slot = u8::try_from(alias % 2).map_err(|_error| IndexError::ResourceLimited)?;
            check_eq!(row.root, u32::from(slot));
            slot
        } else if key == "\0next" {
            check_eq!(row.root, 1);
            64
        } else {
            check_eq!(row.root, 0);
            key.strip_prefix("\0unique-")
                .ok_or(IndexError::ResourceLimited)?
                .parse::<u8>()
                .map_err(|_error| IndexError::CorruptObject("fixture unique key invalid".into()))?
        };
        check_eq!(row.edit.expected, None);
        check_eq!(
            row.edit.next,
            Some(reference(90_u8.checked_add(slot).ok_or(IndexError::ResourceLimited)?).into())
        );
        count = count.checked_add(1).ok_or(IndexError::ResourceLimited)?;
    }
    check_eq!(count, 191);
    drop(cursor);
    fixture.objects.install_outputs = Some(vec![reference(155)]);
    push_fixed(
        &mut writes,
        1,
        b"\0next-again",
        reference(155),
        65,
        &spool,
        &mut edits,
        &mut work,
    )?;
    writes.flush(
        &mut AttemptContext {
            attempt,
            objects: &fixture.objects,
            maps: &fixture.maps,
            custody: &custody,
            scratch: &scratch,
            budget: &mut work,
        },
        &spool,
        &mut confirmed,
        &mut edits,
    )?;
    check_eq!(fixture.objects.install_batches.load(Ordering::Relaxed), 2);
    check_eq!(
        fixture.objects.install_batch_inputs.load(Ordering::Relaxed),
        66
    );
    drop(edits);
    drop(writes);
    drop(confirmed);
    // Packet evidence remains funded by the spool across flushes.
    check!(memory.live.load(Ordering::Relaxed) > 0);
    drop(spool);
    check_eq!(memory.live.load(Ordering::Relaxed), 0);
    check!(!memory.invalid_release.load(Ordering::Relaxed));
    Ok(())
}
