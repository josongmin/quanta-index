//! Direct cold-owner cursor oracles over a fixed packed occurrence block.

use super::super::{
    cold_inventory_cursor::{ColdInventoryCursor, ColdInventoryProvenance},
    entry_map::SmallMap,
};
use super::*;
use quanta_index_core::index::ports::{ImmutableObjectPort, ObjectWriteSink};

struct RetainedRouteSpy<'a> {
    inner: &'a TestObjects,
    counted_reference: ObjectRef,
    get_calls: AtomicUsize,
    retained_calls: AtomicUsize,
    meter_retained_wire: bool,
}

impl ImmutableObjectPort for RetainedRouteSpy<'_> {
    fn identify(
        &self,
        _payload: &[u8],
        _children: &[ObjectRef],
        _budget: &mut WorkBudget,
    ) -> Result<RegisteredObject, IndexError> {
        Err(IndexError::BackendViolation(
            "unexpected cursor object identify".into(),
        ))
    }

    fn put(
        &self,
        _payload: &[u8],
        _children: &[ObjectRef],
        _budget: &mut WorkBudget,
    ) -> Result<RegisteredObject, IndexError> {
        Err(IndexError::BackendViolation(
            "unexpected cursor object put".into(),
        ))
    }

    fn install(
        &self,
        _payload: &[u8],
        _children: &[ObjectRef],
        _sink: &dyn ObjectWriteSink,
        _budget: &mut WorkBudget,
    ) -> Result<RegisteredObject, IndexError> {
        Err(IndexError::BackendViolation(
            "unexpected cursor object install".into(),
        ))
    }

    fn install_batch(
        &self,
        _inputs: &[ObjectBatchInput],
        _sink: &dyn ObjectWriteSink,
        _budget: &mut WorkBudget,
    ) -> Result<quanta_index_core::index::RegisteredObjectBatch, IndexError> {
        Err(IndexError::BackendViolation(
            "unexpected cursor object batch".into(),
        ))
    }

    fn get(
        &self,
        reference: ObjectRef,
        budget: &mut WorkBudget,
    ) -> Result<StoredObject, IndexError> {
        if reference == self.counted_reference {
            let _prior = self.get_calls.fetch_add(1, Ordering::Relaxed);
        }
        self.inner.get(reference, budget)
    }

    fn read_retained(
        &self,
        object: &StoredObject,
        budget: &mut WorkBudget,
    ) -> Result<StoredObject, IndexError> {
        let _prior = self.retained_calls.fetch_add(1, Ordering::Relaxed);
        if self.meter_retained_wire {
            let decoded = object
                .payload
                .len()
                .checked_add(
                    object
                        .registration
                        .children
                        .len()
                        .checked_mul(std::mem::size_of::<ObjectRef>())
                        .ok_or(IndexError::ResourceLimited)?,
                )
                .ok_or(IndexError::ResourceLimited)?;
            budget.charge(WorkUsage {
                reads: 1,
                bytes: object
                    .registration
                    .bytes
                    .checked_add(
                        u64::try_from(decoded).map_err(|_width| IndexError::ResourceLimited)?,
                    )
                    .ok_or(IndexError::ResourceLimited)?,
                ..WorkUsage::default()
            })?;
            return Ok(object.clone());
        }
        // Keep the default adapter's original read and charge in this spy.
        self.inner.get(object.registration.reference, budget)
    }
}

const KEY_SEAL: Digest = Digest([
    0xe4, 0x67, 0x0d, 0x8d, 0x45, 0x7f, 0xa3, 0x66, 0xd5, 0xba, 0x67, 0x1a, 0x48, 0xa5, 0x6f, 0x1e,
    0x0a, 0x13, 0x37, 0x5a, 0x42, 0xe3, 0xa0, 0x1b, 0x85, 0x35, 0xa1, 0x77, 0xc3, 0xd5, 0xe3, 0x3f,
]);

fn fixed_nine_key_block() -> Vec<u8> {
    let mut wire = b"QITXOC01".to_vec();
    wire.extend_from_slice(&9_u16.to_be_bytes());
    wire.extend_from_slice(&1_u16.to_be_bytes());
    wire.extend_from_slice(&0_u32.to_be_bytes());
    for suffix in b'a'..=b'i' {
        wire.extend_from_slice(&2_u16.to_be_bytes());
        wire.extend_from_slice(&[0, suffix]);
        wire.extend_from_slice(&0_u16.to_be_bytes());
    }
    wire.extend_from_slice(&1_u16.to_be_bytes());
    wire.extend_from_slice(&0_u32.to_be_bytes());
    wire
}

#[derive(Clone, Copy)]
enum CursorFault {
    None,
    KeySeal,
    IndexedTokens,
    Attempt,
    TermCount,
    Reference,
}

#[test]
fn cold_cursor_checks_fixed_alias_seal_count_attempt_and_refunds_memory() -> Result<(), IndexError>
{
    for fault in [
        CursorFault::None,
        CursorFault::KeySeal,
        CursorFault::IndexedTokens,
        CursorFault::Attempt,
        CursorFault::TermCount,
        CursorFault::Reference,
    ] {
        let fixture = visitor_fixture()?;
        let mut objects = TestObjects::default();
        let payload = fixed_nine_key_block();
        let registration =
            super::occurrence_blocks::CanonicalObjects::registration(&payload, Vec::new())?;
        let block = registration.reference;
        check!(
            objects
                .stored
                .insert(block, StoredObject::new(registration, payload, None))
                .is_none()
        );
        let memory = Arc::new(TokenMemory {
            ceiling: 512 * 1024 * 1024,
            ..TokenMemory::default()
        });
        let mut work = packet_budget()?;
        work.bind_memory_scope(Some(quanta_index_core::index::MemoryScope::new(Arc::new(
            TokenMemoryScope(Arc::clone(&memory)),
        ))));
        let attempt = test_attempt(fixture.ticket.view.identity.workspace);
        let mut anchor = SmallMap::default();
        check!(anchor.push(b"\0a", block)?);
        let anchor = EntryMap::inline(&anchor);
        let mut next = Contribution {
            record: reference(90),
            inventory: EntryMap::OccurrenceBlock(block),
            indexed_tokens: 9,
        };
        let mut alias = entry_patch::ColdInventoryAlias {
            reference: block,
            terms: 9,
            key_digest: KEY_SEAL,
            anchors: 1,
            descriptor_digest: entry_patch::ColdDescriptorSeal::new().finish(),
            layout: entry_patch::ColdAliasLayout::Inline,
            attempt,
        };
        match fault {
            CursorFault::None => {}
            CursorFault::KeySeal => alias.key_digest = Digest([0; 32]),
            CursorFault::IndexedTokens => next.indexed_tokens = 10,
            CursorFault::Attempt => {
                alias.attempt.fence = alias
                    .attempt
                    .fence
                    .checked_add(1)
                    .ok_or(IndexError::ResourceLimited)?;
            }
            CursorFault::TermCount => alias.terms = 10,
            CursorFault::Reference => alias.reference = reference(91),
        }
        check!(
            ColdInventoryCursor::credit_limits(
                &next,
                Some(&alias),
                None,
                ColdInventoryProvenance::Unsealed,
                &fixture.maps
            )?
            .is_some()
        );
        let scratch = TestScratch::default();
        let custody = TestCustody;
        let mut context = AttemptContext {
            attempt,
            objects: &objects,
            maps: &fixture.maps,
            custody: &custody,
            scratch: &scratch,
            budget: &mut work,
        };
        let alias_input = (alias, Some(anchor));
        let opened = ColdInventoryCursor::open(
            &next,
            Some(&alias_input),
            None,
            ColdInventoryProvenance::Unsealed,
            &objects,
            &fixture.maps,
            attempt,
            context.budget,
        );
        if matches!(fault, CursorFault::Attempt) {
            check!(matches!(opened, Err(IndexError::FenceLost)));
            drop(context);
            check_eq!(memory.live.load(Ordering::Relaxed), 0);
            continue;
        }
        if matches!(fault, CursorFault::Reference) {
            check!(matches!(opened, Err(IndexError::CorruptObject(_))));
            drop(context);
            check_eq!(memory.live.load(Ordering::Relaxed), 0);
            continue;
        }
        let mut cursor = opened?;
        check!(cursor.current()?.is_none());
        let mut observed = 0_u8;
        let result = loop {
            match cursor.advance(&mut context) {
                Ok(true) => {
                    let (key, actual) = cursor.current()?.ok_or(IndexError::ResourceLimited)?;
                    check_eq!(
                        key,
                        &[
                            0,
                            b'a'.checked_add(observed)
                                .ok_or(IndexError::ResourceLimited)?
                        ]
                    );
                    check_eq!(actual, block);
                    observed = observed.checked_add(1).ok_or(IndexError::ResourceLimited)?;
                }
                Ok(false) => break Ok(()),
                Err(error) => break Err(error),
            }
        };
        if matches!(fault, CursorFault::None) {
            result?;
            check_eq!(observed, 9);
            check!(cursor.current()?.is_none());
            check!(!cursor.advance(&mut context)?);
        } else {
            check!(matches!(result, Err(IndexError::CorruptObject(_))));
            check_eq!(
                observed,
                if matches!(fault, CursorFault::TermCount) {
                    0
                } else {
                    9
                }
            );
            check!(matches!(
                cursor.advance(&mut context),
                Err(IndexError::BackendViolation(message))
                    if message == "failed cold cursor reused"
            ));
            check!(matches!(
                cursor.current(),
                Err(IndexError::BackendViolation(_))
            ));
        }
        drop(cursor);
        drop(context);
        check_eq!(memory.live.load(Ordering::Relaxed), 0);
        check!(!memory.invalid_release.load(Ordering::Relaxed));
    }
    Ok(())
}

#[test]
fn prepared_small_cursor_emits_fixed_membership_and_rejects_unsealed_ready_or_false_count()
-> Result<(), IndexError> {
    let fixture = visitor_fixture()?;
    let mut objects = TestObjects::default();
    let payload = fixed_nine_key_block();
    let registration =
        super::occurrence_blocks::CanonicalObjects::registration(&payload, Vec::new())?;
    let block = registration.reference;
    check!(
        objects
            .stored
            .insert(block, StoredObject::new(registration, payload, None))
            .is_none()
    );
    let mut small = SmallMap::default();
    check!(small.push(b"\0a", block)?);
    check!(small.push(b"\0b", block)?);
    let next = Contribution {
        record: reference(90),
        inventory: EntryMap::inline(&small),
        indexed_tokens: 2,
    };
    let spy = RetainedRouteSpy {
        inner: &objects,
        counted_reference: block,
        get_calls: AtomicUsize::new(0),
        retained_calls: AtomicUsize::new(0),
        meter_retained_wire: false,
    };
    let memory = Arc::new(TokenMemory {
        ceiling: 512 * 1024 * 1024,
        ..TokenMemory::default()
    });
    let mut work = packet_budget()?;
    work.bind_memory_scope(Some(quanta_index_core::index::MemoryScope::new(Arc::new(
        TokenMemoryScope(Arc::clone(&memory)),
    ))));
    let attempt = test_attempt(fixture.ticket.view.identity.workspace);
    check!(
        ColdInventoryCursor::credit_limits(
            &next,
            None,
            None,
            ColdInventoryProvenance::PreparedCanonical,
            &fixture.maps,
        )?
        .is_some()
    );
    check!(matches!(
        ColdInventoryCursor::open(
            &next,
            None,
            None,
            ColdInventoryProvenance::Unsealed,
            &spy,
            &fixture.maps,
            attempt,
            &mut work,
        ),
        Err(IndexError::CorruptObject(_))
    ));
    let scratch = TestScratch::default();
    let mut context = AttemptContext {
        attempt,
        objects: &spy,
        maps: &fixture.maps,
        custody: &TestCustody,
        scratch: &scratch,
        budget: &mut work,
    };
    let mut cursor = ColdInventoryCursor::open(
        &next,
        None,
        None,
        ColdInventoryProvenance::PreparedCanonical,
        &spy,
        &fixture.maps,
        attempt,
        context.budget,
    )?;
    for suffix in b'a'..=b'b' {
        let (key, child) = cursor
            .next(&mut context)?
            .ok_or_else(|| IndexError::BackendViolation("prepared small member missing".into()))?;
        check_eq!(key.as_slice(), &[0, suffix]);
        check_eq!(child, block);
    }
    check!(cursor.next(&mut context)?.is_none());
    check!(cursor.next(&mut context)?.is_none());
    check_eq!(spy.get_calls.load(Ordering::Relaxed), 1);
    check_eq!(spy.retained_calls.load(Ordering::Relaxed), 0);
    drop(cursor);
    let false_count = Contribution {
        indexed_tokens: 3,
        ..next
    };
    let mut cursor = ColdInventoryCursor::open(
        &false_count,
        None,
        None,
        ColdInventoryProvenance::PreparedCanonical,
        &spy,
        &fixture.maps,
        attempt,
        context.budget,
    )?;
    check!(cursor.next(&mut context)?.is_some());
    check!(cursor.next(&mut context)?.is_some());
    check!(matches!(
        cursor.next(&mut context),
        Err(IndexError::CorruptObject(_))
    ));
    check!(matches!(
        cursor.next(&mut context),
        Err(IndexError::BackendViolation(message)) if message == "failed cold cursor reused"
    ));
    check_eq!(spy.get_calls.load(Ordering::Relaxed), 2);
    check_eq!(spy.retained_calls.load(Ordering::Relaxed), 0);
    drop(cursor);
    let mut cursor = ColdInventoryCursor::open(
        &next,
        None,
        None,
        ColdInventoryProvenance::PreparedCanonical,
        &spy,
        &fixture.maps,
        attempt,
        context.budget,
    )?;
    check!(cursor.next(&mut context)?.is_some());
    context.attempt.fence = context
        .attempt
        .fence
        .checked_add(1)
        .ok_or(IndexError::ResourceLimited)?;
    check!(matches!(
        cursor.next(&mut context),
        Err(IndexError::FenceLost)
    ));
    context.attempt = attempt;
    check!(matches!(
        cursor.next(&mut context),
        Err(IndexError::BackendViolation(message)) if message == "failed cold cursor reused"
    ));
    check_eq!(spy.get_calls.load(Ordering::Relaxed), 3);
    check_eq!(spy.retained_calls.load(Ordering::Relaxed), 0);
    drop(cursor);
    drop(context);
    check_eq!(memory.live.load(Ordering::Relaxed), 0);
    check!(!memory.invalid_release.load(Ordering::Relaxed));
    Ok(())
}

#[test]
fn borrowed_block_stream_crosses_two_page_boundaries_without_rereading_data()
-> Result<(), IndexError> {
    let mut objects = TestObjects::default();
    let child = reference(91);
    let mut payload = b"QITXEM02".to_vec();
    payload.extend_from_slice(&130_u16.to_be_bytes());
    payload.extend_from_slice(&1_u16.to_be_bytes());
    for ordinal in 0_u16..130 {
        let key = [
            0,
            b't',
            b'0' + u8::try_from(ordinal / 100).map_err(|_| IndexError::ResourceLimited)?,
            b'0' + u8::try_from((ordinal / 10) % 10).map_err(|_| IndexError::ResourceLimited)?,
            b'0' + u8::try_from(ordinal % 10).map_err(|_| IndexError::ResourceLimited)?,
        ];
        payload.extend_from_slice(&5_u16.to_be_bytes());
        payload.extend_from_slice(&key);
        payload.extend_from_slice(&0_u16.to_be_bytes());
    }
    let registration =
        super::occurrence_blocks::CanonicalObjects::registration(&payload, vec![child])?;
    let block = registration.reference;
    let decoded = payload
        .len()
        .checked_add(std::mem::size_of::<ObjectRef>())
        .ok_or(IndexError::ResourceLimited)?;
    let retained_fee = registration
        .bytes
        .checked_add(u64::try_from(decoded).map_err(|_| IndexError::ResourceLimited)?)
        .ok_or(IndexError::ResourceLimited)?;
    check_eq!(retained_fee, 2_444);
    check!(
        objects
            .stored
            .insert(block, StoredObject::new(registration, payload, None))
            .is_none()
    );
    let spy = RetainedRouteSpy {
        inner: &objects,
        counted_reference: block,
        get_calls: AtomicUsize::new(0),
        retained_calls: AtomicUsize::new(0),
        meter_retained_wire: true,
    };
    let mut work = packet_budget()?;
    let mut cursor = entry_map::block::BlockCursor::open(&spy, block, &mut work)?;
    for ordinal in 0_u16..130 {
        let expected = [
            0,
            b't',
            b'0' + u8::try_from(ordinal / 100).map_err(|_| IndexError::ResourceLimited)?,
            b'0' + u8::try_from((ordinal / 10) % 10).map_err(|_| IndexError::ResourceLimited)?,
            b'0' + u8::try_from(ordinal % 10).map_err(|_| IndexError::ResourceLimited)?,
        ];
        check!(
            cursor
                .visit_next(&mut work, |key, reference, _budget| {
                    check_eq!(key, expected.as_slice());
                    check_eq!(reference, child);
                    Ok(())
                })?
                .is_some()
        );
    }
    check!(cursor.visit_next(&mut work, |_, _, _| Ok(()))?.is_none());
    check_eq!(spy.get_calls.load(Ordering::Relaxed), 1);
    check_eq!(spy.retained_calls.load(Ordering::Relaxed), 0);
    check_eq!(work.usage().reads, 1);
    check_eq!(
        work.usage().bytes,
        1_182
            + 130
                * (5 + u64::try_from(std::mem::size_of::<Option<(u16, u16, u16)>>())
                    .map_err(|_| IndexError::ResourceLimited)?)
    );
    let mut retry = entry_map::block::BlockCursor::open(&spy, block, &mut work)?;
    for _ordinal in 0..64 {
        check!(retry.visit_next(&mut work, |_, _, _| Ok(()))?.is_some());
    }
    check_eq!(
        retry.current()?.map(|(key, _)| key),
        Some(b"\0t063".as_slice())
    );
    let mut cancelled = packet_budget()?;
    cancelled.request().cancel_handle().cancel();
    check!(matches!(
        retry.visit_next(&mut cancelled, |_, _, _| Ok(())),
        Err(IndexError::Cancelled)
    ));
    check_eq!(
        retry.current()?.map(|(key, _)| key),
        Some(b"\0t063".as_slice())
    );
    check!(matches!(
        retry.visit_next(&mut work, |_, _, _| Err::<(), _>(
            IndexError::ResourceLimited
        )),
        Err(IndexError::ResourceLimited)
    ));
    check_eq!(
        retry.current()?.map(|(key, _)| key),
        Some(b"\0t063".as_slice())
    );
    check_eq!(
        retry.visit_next(&mut work, |key, _, _| Ok(key.to_vec()))?,
        Some(b"\0t064".to_vec())
    );
    check_eq!(spy.retained_calls.load(Ordering::Relaxed), 0);
    // The former artificial 64-row refreshes cost 2 * 2,444 Work bytes.
    Ok(())
}

#[test]
fn canonical_packed_count_uses_held_data_but_unsealed_count_keeps_read() -> Result<(), IndexError> {
    fn consume(provenance: ColdInventoryProvenance) -> Result<(u64, u64, usize, u64), IndexError> {
        let fixture = visitor_fixture()?;
        let mut objects = TestObjects::default();
        let payload = fixed_nine_key_block();
        let registration =
            super::occurrence_blocks::CanonicalObjects::registration(&payload, Vec::new())?;
        let retained_fee = registration
            .bytes
            .checked_add(u64::try_from(payload.len()).map_err(|_| IndexError::ResourceLimited)?)
            .ok_or(IndexError::ResourceLimited)?;
        let block = registration.reference;
        check!(
            objects
                .stored
                .insert(block, StoredObject::new(registration, payload, None))
                .is_none()
        );
        let spy = RetainedRouteSpy {
            inner: &objects,
            counted_reference: block,
            get_calls: AtomicUsize::new(0),
            retained_calls: AtomicUsize::new(0),
            meter_retained_wire: true,
        };
        let memory = Arc::new(TokenMemory {
            ceiling: 512 * 1024 * 1024,
            ..TokenMemory::default()
        });
        let mut work = packet_budget()?;
        work.bind_memory_scope(Some(quanta_index_core::index::MemoryScope::new(Arc::new(
            TokenMemoryScope(Arc::clone(&memory)),
        ))));
        let attempt = test_attempt(fixture.ticket.view.identity.workspace);
        let mut anchor = SmallMap::default();
        check!(anchor.push(b"\0a", block)?);
        let alias = entry_patch::ColdInventoryAlias {
            reference: block,
            terms: 9,
            key_digest: KEY_SEAL,
            anchors: 1,
            descriptor_digest: entry_patch::ColdDescriptorSeal::new().finish(),
            layout: entry_patch::ColdAliasLayout::Inline,
            attempt,
        };
        let next = Contribution {
            record: reference(90),
            inventory: EntryMap::OccurrenceBlock(block),
            indexed_tokens: 9,
        };
        let scratch = TestScratch::default();
        let mut context = AttemptContext {
            attempt,
            objects: &spy,
            maps: &fixture.maps,
            custody: &TestCustody,
            scratch: &scratch,
            budget: &mut work,
        };
        let mut cursor = ColdInventoryCursor::open(
            &next,
            Some(&(alias, Some(EntryMap::inline(&anchor)))),
            None,
            provenance,
            &spy,
            &fixture.maps,
            attempt,
            context.budget,
        )?;
        for suffix in b'a'..=b'i' {
            check!(cursor.advance(&mut context)?);
            let (key, child) = cursor.current()?.ok_or(IndexError::ResourceLimited)?;
            check_eq!(key, &[0, suffix]);
            check_eq!(child, block);
        }
        check!(!cursor.advance(&mut context)?);
        check!(cursor.current()?.is_none());
        let usage = context.budget.usage();
        let retained = spy.retained_calls.load(Ordering::Relaxed);
        check_eq!(spy.get_calls.load(Ordering::Relaxed), 1);
        drop(cursor);
        drop(context);
        check_eq!(memory.live.load(Ordering::Relaxed), 0);
        Ok((usage.bytes, usage.reads, retained, retained_fee))
    }

    let canonical = consume(ColdInventoryProvenance::PreparedCanonical)?;
    let unsealed = consume(ColdInventoryProvenance::Unsealed)?;
    check_eq!(canonical.2, 0);
    check_eq!(unsealed.2, 1);
    check_eq!(unsealed.1.checked_sub(canonical.1), Some(1));
    check_eq!(unsealed.0.checked_sub(canonical.0), Some(canonical.3));
    check_eq!(canonical.3, 168);
    Ok(())
}

#[test]
fn prepared_directory_without_a_descriptor_rejects_at_eof() -> Result<(), IndexError> {
    let fixture = visitor_fixture()?;
    let objects = TestObjects::default();
    let mut work = packet_budget()?;
    let attempt = test_attempt(fixture.ticket.view.identity.workspace);
    let missing = reference(229);
    let next = Contribution {
        record: reference(90),
        inventory: EntryMap::InventoryDirectory(missing),
        indexed_tokens: 1,
    };
    check!(matches!(
        ColdInventoryCursor::open(
            &next,
            None,
            None,
            ColdInventoryProvenance::Unsealed,
            &objects,
            &fixture.maps,
            attempt,
            &mut work,
        ),
        Err(IndexError::CorruptObject(_))
    ));
    let scratch = TestScratch::default();
    let mut context = AttemptContext {
        attempt,
        objects: &objects,
        maps: &fixture.maps,
        custody: &TestCustody,
        scratch: &scratch,
        budget: &mut work,
    };
    let mut cursor = ColdInventoryCursor::open(
        &next,
        None,
        None,
        ColdInventoryProvenance::PreparedCanonical,
        &objects,
        &fixture.maps,
        attempt,
        context.budget,
    )?;
    check!(matches!(
        cursor.next(&mut context),
        Err(IndexError::CorruptObject(_))
    ));
    check!(matches!(
        cursor.next(&mut context),
        Err(IndexError::BackendViolation(message)) if message == "failed cold cursor reused"
    ));
    Ok(())
}

#[test]
fn prepared_directory_receipt_checks_root_attempt_and_exact_eof_count() -> Result<(), IndexError> {
    let mut fixture = visitor_fixture()?;
    let attempt = test_attempt(fixture.ticket.view.identity.workspace);
    let mut objects = TestObjects::default();
    let payload = fixed_nine_key_block();
    let registration =
        super::occurrence_blocks::CanonicalObjects::registration(&payload, Vec::new())?;
    let block = registration.reference;
    check!(
        objects
            .stored
            .insert(block, StoredObject::new(registration, payload, None))
            .is_none()
    );
    let root = reference(230);
    fixture.maps.add(
        quanta_index_core::index::MapRoot(Some(root)),
        vec![0, b'i'],
        quanta_index_core::index::MapValue::inline(&[1, 0, 9], [block])?,
    )?;
    let next = Contribution {
        record: reference(90),
        inventory: EntryMap::InventoryDirectory(root),
        indexed_tokens: 9,
    };
    let receipt = entry_patch::DirectoryReceipt {
        root,
        descriptors: 1,
        attempt,
        slot: 0,
    };
    let mut work = packet_budget()?;
    let wrong_root = entry_patch::DirectoryReceipt {
        root: reference(231),
        ..receipt
    };
    check!(matches!(
        ColdInventoryCursor::open(
            &next,
            None,
            Some(&wrong_root),
            ColdInventoryProvenance::PreparedCanonical,
            &objects,
            &fixture.maps,
            attempt,
            &mut work
        ),
        Err(IndexError::CorruptObject(_))
    ));
    let wrong_attempt = entry_patch::DirectoryReceipt {
        attempt: quanta_index_core::index::AttemptRef {
            fence: attempt.fence + 1,
            ..attempt
        },
        ..receipt
    };
    check!(matches!(
        ColdInventoryCursor::open(
            &next,
            None,
            Some(&wrong_attempt),
            ColdInventoryProvenance::PreparedCanonical,
            &objects,
            &fixture.maps,
            attempt,
            &mut work
        ),
        Err(IndexError::FenceLost)
    ));
    let scratch = TestScratch::default();
    let mut context = AttemptContext {
        attempt,
        objects: &objects,
        maps: &fixture.maps,
        custody: &TestCustody,
        scratch: &scratch,
        budget: &mut work,
    };
    let mut cursor = ColdInventoryCursor::open(
        &next,
        None,
        Some(&receipt),
        ColdInventoryProvenance::PreparedCanonical,
        &objects,
        &fixture.maps,
        attempt,
        context.budget,
    )?;
    for suffix in b'a'..=b'i' {
        let (key, reference) = cursor
            .next(&mut context)?
            .ok_or(IndexError::ResourceLimited)?;
        check_eq!(key.as_slice(), &[0, suffix]);
        check_eq!(reference, block);
    }
    check!(cursor.next(&mut context)?.is_none());
    drop(cursor);
    let wrong_count = entry_patch::DirectoryReceipt {
        descriptors: 2,
        ..receipt
    };
    let mut cursor = ColdInventoryCursor::open(
        &next,
        None,
        Some(&wrong_count),
        ColdInventoryProvenance::PreparedCanonical,
        &objects,
        &fixture.maps,
        attempt,
        context.budget,
    )?;
    for _ in 0..9 {
        check!(cursor.next(&mut context)?.is_some());
    }
    check!(matches!(
        cursor.next(&mut context),
        Err(IndexError::CorruptObject(_))
    ));
    Ok(())
}

#[test]
fn prepared_map_block_reuses_checked_occurrence_across_nine_selectors() -> Result<(), IndexError> {
    let fixture = visitor_fixture()?;
    let mut objects = TestObjects::default();
    let occurrence_payload = fixed_nine_key_block();
    let occurrence_registration =
        super::occurrence_blocks::CanonicalObjects::registration(&occurrence_payload, Vec::new())?;
    let occurrence = occurrence_registration.reference;
    check!(
        objects
            .stored
            .insert(
                occurrence,
                StoredObject::new(occurrence_registration, occurrence_payload, None),
            )
            .is_none()
    );
    let mut map_payload = b"QITXEM02".to_vec();
    map_payload.extend_from_slice(&9_u16.to_be_bytes());
    map_payload.extend_from_slice(&1_u16.to_be_bytes());
    for suffix in b'a'..=b'i' {
        map_payload.extend_from_slice(&2_u16.to_be_bytes());
        map_payload.extend_from_slice(&[0, suffix]);
        map_payload.extend_from_slice(&0_u16.to_be_bytes());
    }
    let map_registration =
        super::occurrence_blocks::CanonicalObjects::registration(&map_payload, vec![occurrence])?;
    let map_block = map_registration.reference;
    check!(
        objects
            .stored
            .insert(
                map_block,
                StoredObject::new(map_registration, map_payload, None),
            )
            .is_none()
    );
    let spy = RetainedRouteSpy {
        inner: &objects,
        counted_reference: occurrence,
        get_calls: AtomicUsize::new(0),
        retained_calls: AtomicUsize::new(0),
        meter_retained_wire: false,
    };
    let memory = Arc::new(TokenMemory {
        ceiling: 512 * 1024 * 1024,
        ..TokenMemory::default()
    });
    let mut work = packet_budget()?;
    work.bind_memory_scope(Some(quanta_index_core::index::MemoryScope::new(Arc::new(
        TokenMemoryScope(Arc::clone(&memory)),
    ))));
    let attempt = test_attempt(fixture.ticket.view.identity.workspace);
    let next = Contribution {
        record: reference(90),
        inventory: EntryMap::Block(map_block),
        indexed_tokens: 9,
    };
    check!(
        ColdInventoryCursor::credit_limits(
            &next,
            None,
            None,
            ColdInventoryProvenance::PreparedCanonical,
            &fixture.maps,
        )?
        .is_some()
    );
    let scratch = TestScratch::default();
    let mut context = AttemptContext {
        attempt,
        objects: &spy,
        maps: &fixture.maps,
        custody: &TestCustody,
        scratch: &scratch,
        budget: &mut work,
    };
    let mut cursor = ColdInventoryCursor::open(
        &next,
        None,
        None,
        ColdInventoryProvenance::PreparedCanonical,
        &spy,
        &fixture.maps,
        attempt,
        context.budget,
    )?;
    for suffix in b'a'..=b'i' {
        let (key, child) = cursor.next(&mut context)?.ok_or_else(|| {
            IndexError::BackendViolation("prepared map-block member missing".into())
        })?;
        check_eq!(key.as_slice(), &[0, suffix]);
        check_eq!(child, occurrence);
    }
    check!(cursor.next(&mut context)?.is_none());
    check_eq!(spy.get_calls.load(Ordering::Relaxed), 1);
    check_eq!(spy.retained_calls.load(Ordering::Relaxed), 0);
    drop(cursor);
    drop(context);
    check_eq!(memory.live.load(Ordering::Relaxed), 0);
    check!(!memory.invalid_release.load(Ordering::Relaxed));
    Ok(())
}
