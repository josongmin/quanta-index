//! A cold alias is not an owner transition until every owner in its window
//! has passed immutable readback and the complete diff prepare.

use super::super::entry_map::SmallMap;
use super::*;

#[derive(Default)]
struct ObservedObjects {
    inner: TestObjects,
    physical_gets: AtomicUsize,
    retained_reads: AtomicUsize,
    refuse_retained: AtomicBool,
    corrupt_retained: AtomicBool,
}

impl ObservedObjects {
    fn add_canonical_block(&mut self, payload: Vec<u8>) -> Result<ObjectRef, IndexError> {
        let registration =
            super::occurrence_blocks::CanonicalObjects::registration(&payload, Vec::new())?;
        let reference = registration.reference;
        if self
            .inner
            .stored
            .insert(reference, StoredObject::new(registration, payload, None))
            .is_some()
        {
            return Err(IndexError::BackendViolation(
                "canonical block fixture reused an identity".into(),
            ));
        }
        Ok(reference)
    }
}

impl ImmutableObjectPort for ObservedObjects {
    fn identify(
        &self,
        payload: &[u8],
        children: &[ObjectRef],
        budget: &mut WorkBudget,
    ) -> Result<RegisteredObject, IndexError> {
        self.inner.identify(payload, children, budget)
    }

    fn put(
        &self,
        payload: &[u8],
        children: &[ObjectRef],
        budget: &mut WorkBudget,
    ) -> Result<RegisteredObject, IndexError> {
        self.inner.put(payload, children, budget)
    }

    fn install(
        &self,
        payload: &[u8],
        children: &[ObjectRef],
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<RegisteredObject, IndexError> {
        self.inner.install(payload, children, sink, budget)
    }

    fn install_batch(
        &self,
        inputs: &[ObjectBatchInput],
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<quanta_index_core::index::RegisteredObjectBatch, IndexError> {
        self.inner.install_batch(inputs, sink, budget)
    }

    fn get(
        &self,
        reference: ObjectRef,
        budget: &mut WorkBudget,
    ) -> Result<StoredObject, IndexError> {
        let _old = self.physical_gets.fetch_add(1, Ordering::Relaxed);
        self.inner.get(reference, budget)
    }

    fn read_retained(
        &self,
        object: &StoredObject,
        budget: &mut WorkBudget,
    ) -> Result<StoredObject, IndexError> {
        // Mirror the selected ObjectStore adapter's read/wire/decode charge
        // order. This fixture tests the builder boundary, not storage I/O.
        budget.charge(WorkUsage {
            reads: 1,
            ..WorkUsage::default()
        })?;
        budget.charge(WorkUsage {
            bytes: object.registration.bytes,
            ..WorkUsage::default()
        })?;
        let decoded = object
            .registration
            .children
            .len()
            .checked_mul(std::mem::size_of::<ObjectRef>())
            .and_then(|children| children.checked_add(object.payload.len()))
            .ok_or(IndexError::ResourceLimited)?;
        budget.charge(WorkUsage {
            bytes: u64::try_from(decoded).map_err(|_error| IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;
        let _old = self.retained_reads.fetch_add(1, Ordering::Relaxed);
        if self.refuse_retained.load(Ordering::Relaxed) {
            return Err(IndexError::ResourceLimited);
        }
        if self.corrupt_retained.load(Ordering::Relaxed) {
            let mut payload = object.payload.clone();
            let byte = payload.first_mut().ok_or(IndexError::ResourceLimited)?;
            *byte ^= 1;
            return Ok(StoredObject::new(
                object.registration.clone(),
                payload,
                None,
            ));
        }
        Ok(object.clone())
    }
}

/// Independent QITXOC01 wire: nine sorted case-prefixed keys share one
/// position vector. This is a fixed input, not output from the builder under
/// test.
fn nine_key_block(position: u32) -> Vec<u8> {
    let mut wire = b"QITXOC01".to_vec();
    wire.extend_from_slice(&9_u16.to_be_bytes());
    wire.extend_from_slice(&1_u16.to_be_bytes());
    wire.extend_from_slice(&0_u32.to_be_bytes());
    for suffix in b'a'..=b'i' {
        wire.extend_from_slice(b"\x00\x02\x00");
        wire.push(suffix);
        wire.extend_from_slice(&0_u16.to_be_bytes());
    }
    wire.extend_from_slice(&1_u16.to_be_bytes());
    wire.extend_from_slice(&position.to_be_bytes());
    wire
}

fn nine_key_seal(budget: &mut WorkBudget) -> Result<Digest, IndexError> {
    let mut seal = entry_patch::ColdInventoryKeySeal::new();
    for suffix in b'a'..=b'i' {
        seal.push(&[0, suffix], budget)?;
    }
    Ok(seal.finish())
}

#[derive(Clone, Copy)]
enum TailFault {
    Seal,
    Count,
    Reference,
    Tokens,
}

fn staged_two_aliases(
    attempt: AttemptRef,
    first: ObjectRef,
    second: ObjectRef,
    seal: Digest,
    fault: Option<TailFault>,
    budget: &mut WorkBudget,
) -> Result<InventoryBatch<'static>, IndexError> {
    let mut batch = InventoryBatch::cold(2)?;
    batch.occurrence_writes.pack_owners = true;
    batch.attempt = Some(attempt);
    for (slot, anchor_ref) in [first, second].into_iter().enumerate() {
        let mut alias = entry_patch::ColdInventoryAlias {
            reference: anchor_ref,
            terms: 9,
            key_digest: seal,
            anchors: 1,
            descriptor_digest: entry_patch::ColdDescriptorSeal::new().finish(),
            layout: entry_patch::ColdAliasLayout::Inline,
            attempt,
        };
        if slot == 1 {
            match fault {
                Some(TailFault::Seal) => alias.key_digest = Digest([0; 32]),
                Some(TailFault::Count) => alias.terms = 10,
                Some(TailFault::Reference) => alias.reference = reference(250),
                Some(TailFault::Tokens) => {}
                None => {}
            }
        }
        batch.rows.push(
            PendingTextChange {
                owner: [u8::try_from(slot + 1).map_err(|_error| IndexError::ResourceLimited)?; 32],
                prior_ref: None,
                prior: None,
                next: PendingTextNext::Draft {
                    record: reference(
                        u8::try_from(100 + slot).map_err(|_error| IndexError::ResourceLimited)?,
                    ),
                    slot,
                    body: TextBodyKey {
                        bytes: reference(
                            u8::try_from(120 + slot)
                                .map_err(|_error| IndexError::ResourceLimited)?,
                        ),
                        byte_len: 0,
                    },
                    tokens: if slot == 1 && matches!(fault, Some(TailFault::Tokens)) {
                        10
                    } else {
                        9
                    },
                    alias: Some(alias),
                },
            },
            budget,
        )?;
        batch.ready.push(ReadyTextNext::default(), budget)?;
        batch.bases.push(EntryMap::default(), budget)?;
        batch.counts.push(1, budget)?;
        batch.inventory_edits.push_partition(
            InventoryPartition::Ready(
                u32::try_from(slot).map_err(|_error| IndexError::ResourceLimited)?,
            ),
            PreparedKey::copy(b"\x00a", budget)?,
            None,
            Some(anchor_ref.into()),
            budget,
        )?;
    }
    Ok(batch)
}

#[test]
fn second_owner_alias_or_diff_fault_keeps_effects_uncommitted_and_poisons_reuse()
-> Result<(), IndexError> {
    for fault in [
        TailFault::Seal,
        TailFault::Count,
        TailFault::Reference,
        // The alias and readback are valid; the second Diff's token total is
        // wrong after the first owner's entire Diff prepared successfully.
        TailFault::Tokens,
    ] {
        let fixture = visitor_fixture()?;
        let mut objects = ObservedObjects::default();
        let first = objects.add_canonical_block(nine_key_block(0))?;
        let second = objects.add_canonical_block(nine_key_block(1))?;
        let scratch = TestScratch::default();
        let custody = TestCustody;
        let mut work = packet_budget()?;
        let seal = nine_key_seal(&mut work)?;
        let spool = TextSpool::new(&scratch, &mut work)?;
        let attempt = test_attempt(fixture.ticket.view.identity.workspace);
        let mut batch = staged_two_aliases(attempt, first, second, seal, Some(fault), &mut work)?;
        let mut root = Root::default();
        let mut contributions = ContributionWrites::cold()?;
        let mut inventories = InventoryReuse::new(fixture.recipe, 64)?;
        let mut context = AttemptContext {
            attempt,
            objects: &objects,
            maps: &fixture.maps,
            custody: &custody,
            scratch: &scratch,
            budget: &mut work,
        };
        check!(matches!(
            batch.flush(
                &mut root,
                &mut context,
                &spool,
                &mut contributions,
                &mut inventories,
            ),
            Err(IndexError::CorruptObject(_))
        ));
        check_eq!(root, Root::default());
        check_eq!(contributions.pending.len(), 0);
        check_eq!(objects.inner.installs.load(Ordering::Relaxed), 0);
        check_eq!(fixture.maps.map_applies.load(Ordering::Relaxed), 0);
        check!(spool.top_cursor().next_edit(context.budget).is_err());
        check!(
            spool
                .inventory_cursor(2)?
                .next_edit(context.budget)
                .is_err()
        );
        check!(
            batch
                .flush(
                    &mut root,
                    &mut context,
                    &spool,
                    &mut contributions,
                    &mut inventories,
                )
                .is_err()
        );
        check_eq!(root, Root::default());
        check_eq!(contributions.pending.len(), 0);
        check!(spool.top_cursor().next_edit(context.budget).is_err());
    }
    Ok(())
}

#[test]
fn valid_two_owner_aliases_read_each_physical_block_once() -> Result<(), IndexError> {
    let fixture = visitor_fixture()?;
    let mut objects = ObservedObjects::default();
    let first = objects.add_canonical_block(nine_key_block(0))?;
    let second = objects.add_canonical_block(nine_key_block(1))?;
    let scratch = TestScratch::default();
    let custody = TestCustody;
    let mut work = packet_budget()?;
    let seal = nine_key_seal(&mut work)?;
    let spool = TextSpool::new(&scratch, &mut work)?;
    let attempt = test_attempt(fixture.ticket.view.identity.workspace);
    let mut batch = staged_two_aliases(attempt, first, second, seal, None, &mut work)?;
    let mut root = Root::default();
    let mut contributions = ContributionWrites::cold()?;
    let mut inventories = InventoryReuse::new(fixture.recipe, 64)?;
    let mut context = AttemptContext {
        attempt,
        objects: &objects,
        maps: &fixture.maps,
        custody: &custody,
        scratch: &scratch,
        budget: &mut work,
    };
    batch.flush(
        &mut root,
        &mut context,
        &spool,
        &mut contributions,
        &mut inventories,
    )?;
    check_eq!(root.docs, 2);
    check_eq!(root.indexed_tokens, 18);
    check_eq!(objects.physical_gets.load(Ordering::Relaxed), 2);
    // Each owner authenticates one physical block and crosses one independent
    // first-count admission while the same checked keys feed seal and diff.
    check_eq!(objects.retained_reads.load(Ordering::Relaxed), 2);
    Ok(())
}

#[test]
fn retained_fixture_charges_fixed_wire_and_decode_bytes_before_returning_data()
-> Result<(), IndexError> {
    let mut objects = ObservedObjects::default();
    let block = objects.add_canonical_block(nine_key_block(0))?;
    let stored = objects
        .inner
        .stored
        .get(&block)
        .cloned()
        .ok_or_else(|| IndexError::CorruptObject("canonical block fixture missing".into()))?;
    // Independent QITXOC01 fixture: 16-byte header + 9 six-byte rows +
    // 6-byte position vector. Canonical object wire adds 16 bytes.
    check_eq!(stored.payload.len(), 76);
    check_eq!(stored.registration.bytes, 92);
    let mut work = packet_budget()?;
    let before = work.usage();
    check_eq!(objects.read_retained(&stored, &mut work)?, stored);
    let after = work.usage();
    check_eq!(after.reads - before.reads, 1);
    check_eq!(after.bytes - before.bytes, 168);
    check_eq!(after.writes, before.writes);
    check_eq!(after.mutations, before.mutations);
    check_eq!(objects.retained_reads.load(Ordering::Relaxed), 1);

    let deadline = Instant::now()
        .checked_add(Duration::from_secs(30))
        .ok_or(IndexError::ResourceLimited)?;
    let mut short = WorkBudget::new(
        WorkLimits {
            reads: 1,
            writes: 0,
            bytes: 167,
            mutations: 0,
        },
        deadline,
    );
    check!(matches!(
        objects.read_retained(&stored, &mut short),
        Err(IndexError::ResourceLimited)
    ));
    // The final decode charge is denied. Successful preceding charges remain
    // visible, and no retained DATA is returned after the refusal.
    check_eq!(short.usage().reads, 1);
    check_eq!(short.usage().bytes, 92);
    check_eq!(objects.retained_reads.load(Ordering::Relaxed), 1);
    Ok(())
}

#[test]
fn retained_read_refusal_or_changed_data_cannot_complete_a_cold_owner() -> Result<(), IndexError> {
    for corrupt in [false, true] {
        let fixture = visitor_fixture()?;
        let mut objects = ObservedObjects::default();
        let block = objects.add_canonical_block(nine_key_block(0))?;
        objects.refuse_retained.store(!corrupt, Ordering::Relaxed);
        objects.corrupt_retained.store(corrupt, Ordering::Relaxed);
        let scratch = TestScratch::default();
        let custody = TestCustody;
        let mut work = packet_budget()?;
        let seal = nine_key_seal(&mut work)?;
        let spool = TextSpool::new(&scratch, &mut work)?;
        let attempt = test_attempt(fixture.ticket.view.identity.workspace);
        let mut batch = staged_two_aliases(attempt, block, block, seal, None, &mut work)?;
        let mut root = Root::default();
        let mut contributions = ContributionWrites::cold()?;
        let mut inventories = InventoryReuse::new(fixture.recipe, 64)?;
        let mut context = AttemptContext {
            attempt,
            objects: &objects,
            maps: &fixture.maps,
            custody: &custody,
            scratch: &scratch,
            budget: &mut work,
        };
        let result = batch.flush(
            &mut root,
            &mut context,
            &spool,
            &mut contributions,
            &mut inventories,
        );
        if corrupt {
            check!(matches!(result, Err(IndexError::CorruptObject(_))));
        } else {
            check!(matches!(result, Err(IndexError::ResourceLimited)));
        }
        check_eq!(objects.physical_gets.load(Ordering::Relaxed), 1);
        check_eq!(objects.retained_reads.load(Ordering::Relaxed), 1);
        check_eq!(root, Root::default());
        check_eq!(contributions.pending.len(), 0);
        check!(spool.top_cursor().next_edit(context.budget).is_err());
        check!(
            spool
                .inventory_cursor(2)?
                .next_edit(context.budget)
                .is_err()
        );
        check!(
            batch
                .flush(
                    &mut root,
                    &mut context,
                    &spool,
                    &mut contributions,
                    &mut inventories,
                )
                .is_err()
        );
    }
    Ok(())
}

#[test]
fn cancellation_after_alias_get_poisoned_window_rejects_fresh_budget_reuse()
-> Result<(), IndexError> {
    let fixture = visitor_fixture()?;
    let mut objects = ObservedObjects::default();
    let first = objects.add_canonical_block(nine_key_block(0))?;
    let second = objects.add_canonical_block(nine_key_block(1))?;
    objects.inner.cancel_after_get = Some(first);
    let scratch = TestScratch::default();
    let custody = TestCustody;
    let mut work = packet_budget()?;
    let seal = nine_key_seal(&mut work)?;
    let spool = TextSpool::new(&scratch, &mut work)?;
    let attempt = test_attempt(fixture.ticket.view.identity.workspace);
    let mut batch = staged_two_aliases(attempt, first, second, seal, None, &mut work)?;
    let mut root = Root::default();
    let mut contributions = ContributionWrites::cold()?;
    let mut inventories = InventoryReuse::new(fixture.recipe, 64)?;
    let mut context = AttemptContext {
        attempt,
        objects: &objects,
        maps: &fixture.maps,
        custody: &custody,
        scratch: &scratch,
        budget: &mut work,
    };
    check!(matches!(
        batch.flush(
            &mut root,
            &mut context,
            &spool,
            &mut contributions,
            &mut inventories,
        ),
        Err(IndexError::Cancelled)
    ));
    check_eq!(root, Root::default());
    check_eq!(contributions.pending.len(), 0);
    check_eq!(objects.physical_gets.load(Ordering::Relaxed), 1);
    let mut fresh = packet_budget()?;
    check!(spool.top_cursor().next_edit(&mut fresh).is_err());
    check!(spool.inventory_cursor(2)?.next_edit(&mut fresh).is_err());
    let mut fresh_context = AttemptContext {
        attempt,
        objects: &objects,
        maps: &fixture.maps,
        custody: &custody,
        scratch: &scratch,
        budget: &mut fresh,
    };
    check!(
        batch
            .flush(
                &mut root,
                &mut fresh_context,
                &spool,
                &mut contributions,
                &mut inventories,
            )
            .is_err()
    );
    Ok(())
}

#[test]
fn cold_alias_cannot_cross_attempt_before_physical_read() -> Result<(), IndexError> {
    let fixture = visitor_fixture()?;
    let mut objects = ObservedObjects::default();
    let block = objects.add_canonical_block(nine_key_block(0))?;
    let scratch = TestScratch::default();
    let custody = TestCustody;
    let mut work = packet_budget()?;
    let seal = nine_key_seal(&mut work)?;
    let attempt = test_attempt(fixture.ticket.view.identity.workspace);
    let mut anchor = SmallMap::default();
    check!(anchor.push(b"\x00a", block)?);
    let alias = entry_patch::ColdInventoryAlias {
        reference: block,
        terms: 9,
        key_digest: seal,
        anchors: 1,
        descriptor_digest: entry_patch::ColdDescriptorSeal::new().finish(),
        layout: entry_patch::ColdAliasLayout::Inline,
        attempt,
    };
    let context = AttemptContext {
        attempt,
        objects: &objects,
        maps: &fixture.maps,
        custody: &custody,
        scratch: &scratch,
        budget: &mut work,
    };
    let mut foreign = attempt;
    foreign.attempt = AttemptId([35; 16]);
    check!(matches!(
        alias.visit_inline(
            EntryMap::OccurrenceBlock(block),
            &EntryMap::inline(&anchor),
            context.objects,
            foreign,
            context.budget,
            |_, _, _, _| Ok(()),
        ),
        Err(IndexError::FenceLost)
    ));
    check_eq!(objects.physical_gets.load(Ordering::Relaxed), 0);
    check_eq!(objects.retained_reads.load(Ordering::Relaxed), 0);
    Ok(())
}

#[test]
fn buffered_top_cursor_and_writer_refuse_after_inventory_invalidation_without_new_work()
-> Result<(), IndexError> {
    let scratch = TestScratch::default();
    let mut work = packet_budget()?;
    let spool = TextSpool::new(&scratch, &mut work)?;
    // An ordinary empty top scan completes successfully. Poison is a
    // different result even if no further row may be exposed.
    check!(spool.top_cursor().next_edit(&mut work)?.is_none());
    spool.top_batch(
        &[
            (
                0,
                MapEdit::objects(b"\x00alpha".to_vec(), None, Some(reference(181))),
            ),
            (
                0,
                MapEdit::objects(b"\x00beta".to_vec(), None, Some(reference(182))),
            ),
        ],
        &mut work,
    )?;
    let mut cursor = spool.top_cursor();
    let first = cursor.next_edit(&mut work)?.ok_or_else(|| {
        IndexError::BackendViolation("two-row top fixture yielded no first row".into())
    })?;
    check_eq!(first.key(), b"\x00alpha".to_vec());
    // The page fetch admitted both rows. The next row is already buffered,
    // but the invalidated attempt may not expose it or a packing decision.
    spool.invalidate_inventory();
    let fee_before = work.usage();
    check!(matches!(
        cursor.next_edit(&mut work),
        Err(IndexError::BackendViolation(_))
    ));
    check!(matches!(
        cursor.subtree_packing(),
        Err(IndexError::BackendViolation(_))
    ));
    check!(matches!(
        spool.top_batch(
            &[(
                0,
                MapEdit::objects(b"\x00gamma".to_vec(), None, Some(reference(183)))
            )],
            &mut work,
        ),
        Err(IndexError::BackendViolation(_))
    ));
    check_eq!(work.usage(), fee_before);
    Ok(())
}

#[test]
fn dropping_prepared_diff_poisoned_batch_and_spool_before_any_owner_transition()
-> Result<(), IndexError> {
    let fixture = visitor_fixture()?;
    let mut objects = ObservedObjects::default();
    let first = objects.add_canonical_block(nine_key_block(0))?;
    let second = objects.add_canonical_block(nine_key_block(1))?;
    let scratch = TestScratch::default();
    let custody = TestCustody;
    let mut work = packet_budget()?;
    let seal = nine_key_seal(&mut work)?;
    let spool = TextSpool::new(&scratch, &mut work)?;
    let attempt = test_attempt(fixture.ticket.view.identity.workspace);
    let mut batch = staged_two_aliases(attempt, first, second, seal, None, &mut work)?;
    batch.inventory_edits.flush(&spool, &mut work)?;
    let mut root = Root::default();
    let mut contributions = ContributionWrites::cold()?;
    let mut inventories = InventoryReuse::new(fixture.recipe, 64)?;
    let mut context = AttemptContext {
        attempt,
        objects: &objects,
        maps: &fixture.maps,
        custody: &custody,
        scratch: &scratch,
        budget: &mut work,
    };
    let mut roots = entry_patch::apply(
        super::super::entry_map::EntryMapUse::ColdInventory,
        batch.bases.as_slice(),
        batch.counts.as_slice(),
        &mut spool.inventory_cursor(batch.bases.len())?,
        |slot| spool.inventory_root_cursor(slot),
        &mut context,
        None,
    )?;
    let prepared = super::super::prepared_inventory::PreparedInventoryChanges::prepare(
        &mut batch,
        Some(roots.as_mut_slice()),
        &mut context,
        &spool,
        &mut inventories,
    )?;
    check_eq!(objects.physical_gets.load(Ordering::Relaxed), 2);
    check_eq!(root, Root::default());
    check_eq!(contributions.pending.len(), 0);
    drop(prepared);
    check!(batch.failed);
    check!(spool.top_cursor().next_edit(context.budget).is_err());
    check!(
        spool
            .inventory_cursor(2)?
            .next_edit(context.budget)
            .is_err()
    );
    check!(matches!(
        batch.flush(
            &mut root,
            &mut context,
            &spool,
            &mut contributions,
            &mut inventories,
        ),
        Err(IndexError::BackendViolation(_))
    ));
    check_eq!(root, Root::default());
    check_eq!(contributions.pending.len(), 0);
    check_eq!(objects.inner.installs.load(Ordering::Relaxed), 0);
    check_eq!(fixture.maps.map_applies.load(Ordering::Relaxed), 0);
    Ok(())
}

#[test]
fn second_completion_failure_poisoned_scratch_after_first_owner_local_effect()
-> Result<(), IndexError> {
    let fixture = visitor_fixture()?;
    let mut objects = ObservedObjects::default();
    let first = objects.add_canonical_block(nine_key_block(0))?;
    let second = objects.add_canonical_block(nine_key_block(1))?;
    let scratch = TestScratch::default();
    let custody = TestCustody;
    let mut work = packet_budget()?;
    let seal = nine_key_seal(&mut work)?;
    let spool = TextSpool::new(&scratch, &mut work)?;
    let attempt = test_attempt(fixture.ticket.view.identity.workspace);
    let mut batch = staged_two_aliases(attempt, first, second, seal, None, &mut work)?;
    batch.inventory_edits.flush(&spool, &mut work)?;
    let mut inventories = InventoryReuse::new(fixture.recipe, 64)?;
    let mut context = AttemptContext {
        attempt,
        objects: &objects,
        maps: &fixture.maps,
        custody: &custody,
        scratch: &scratch,
        budget: &mut work,
    };
    let mut roots = entry_patch::apply(
        super::super::entry_map::EntryMapUse::ColdInventory,
        batch.bases.as_slice(),
        batch.counts.as_slice(),
        &mut spool.inventory_cursor(batch.bases.len())?,
        |slot| spool.inventory_root_cursor(slot),
        &mut context,
        None,
    )?;
    let prepared = super::super::prepared_inventory::PreparedInventoryChanges::prepare(
        &mut batch,
        Some(roots.as_mut_slice()),
        &mut context,
        &spool,
        &mut inventories,
    )?;
    // packet_budget has a fixed 1,024-write grant. After successful prepare,
    // leave exactly one write for the first owner's one-row Top packet. The
    // second Top packet must encounter the original budget's write refusal.
    const PACKET_WRITE_LIMIT: u64 = 1024;
    let remaining = PACKET_WRITE_LIMIT
        .checked_sub(context.budget.usage().writes)
        .ok_or(IndexError::ResourceLimited)?;
    let precharge = remaining
        .checked_sub(1)
        .ok_or(IndexError::ResourceLimited)?;
    context.budget.charge(WorkUsage {
        writes: precharge,
        ..WorkUsage::default()
    })?;
    check_eq!(context.budget.usage().writes, PACKET_WRITE_LIMIT - 1);
    let mut contributions = ContributionWrites::with_limit(1)?;
    let mut root = Root::default();
    check!(matches!(
        prepared.complete(
            &mut root,
            &mut context,
            &mut contributions,
            &mut inventories
        ),
        Err(IndexError::ResourceLimited)
    ));
    check_eq!(root.docs, 1);
    check_eq!(root.indexed_tokens, 9);
    check_eq!(contributions.pending.len(), 1);
    check_eq!(context.budget.usage().writes, PACKET_WRITE_LIMIT);
    check_eq!(
        context.budget.first_denial(),
        Some(quanta_index_core::index::WorkBudgetDenial::Writes)
    );
    check!(batch.failed);
    check!(matches!(
        spool.top_cursor().next_edit(context.budget),
        Err(IndexError::BackendViolation(_))
    ));
    check!(matches!(
        spool.posting_term_page(None, context.budget),
        Err(IndexError::BackendViolation(_))
    ));
    check!(matches!(
        spool.inventory_cursor(2)?.next_edit(context.budget),
        Err(IndexError::BackendViolation(_))
    ));
    check_eq!(fixture.maps.map_applies.load(Ordering::Relaxed), 0);
    check_eq!(objects.inner.installs.load(Ordering::Relaxed), 0);
    Ok(())
}
