//! Confirmed occurrence spill changes physical puts, not inventory authority.

use super::*;
use quanta_index_core::index::{RegisteredObjectBatch, WorkLimits};

#[derive(Default)]
struct CountedOccurrences {
    physical_inputs: AtomicUsize,
    per_identity: Mutex<BTreeMap<ObjectRef, usize>>,
}

impl ImmutableObjectPort for CountedOccurrences {
    fn identify(
        &self,
        payload: &[u8],
        children: &[ObjectRef],
        budget: &mut WorkBudget,
    ) -> Result<RegisteredObject, IndexError> {
        budget.checkpoint()?;
        let bytes = u64::try_from(payload.len()).map_err(|_error| IndexError::ResourceLimited)?;
        Ok(RegisteredObject::unscoped(
            ObjectRef(Digest(Sha256::digest(payload).into())),
            bytes,
            children.to_vec(),
        ))
    }

    fn put(
        &self,
        _payload: &[u8],
        _children: &[ObjectRef],
        _budget: &mut WorkBudget,
    ) -> Result<RegisteredObject, IndexError> {
        Err(IndexError::BackendViolation(
            "unexpected scalar occurrence put".into(),
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
            "unexpected scalar occurrence install".into(),
        ))
    }

    fn install_batch(
        &self,
        inputs: &[ObjectBatchInput],
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<RegisteredObjectBatch, IndexError> {
        budget.checkpoint()?;
        let registrations: Vec<RegisteredObject> = inputs
            .iter()
            .map(|input| self.identify(&input.payload, &input.children, budget))
            .collect::<Result<_, _>>()?;
        sink.reserve_batch(&registrations)?;
        sink.installed_batch(&registrations)?;
        let _prior = self
            .physical_inputs
            .fetch_add(registrations.len(), Ordering::Relaxed);
        let mut counts = self
            .per_identity
            .lock()
            .map_err(|_error| IndexError::Conflict)?;
        for registration in &registrations {
            let count = counts.entry(registration.reference).or_default();
            *count = count.checked_add(1).ok_or(IndexError::ResourceLimited)?;
        }
        drop(counts);
        Ok(RegisteredObjectBatch::unscoped(registrations))
    }

    fn get(
        &self,
        _reference: ObjectRef,
        _budget: &mut WorkBudget,
    ) -> Result<StoredObject, IndexError> {
        Err(IndexError::BackendViolation(
            "unexpected occurrence read in new-inventory preparation".into(),
        ))
    }
}

type InventoryGold = Vec<(u32, Vec<u8>, Option<MapValue>)>;

fn two_owner_inventory(
    reuse_limit: usize,
) -> Result<(usize, Vec<usize>, InventoryGold, u64), IndexError> {
    let deadline = Instant::now()
        .checked_add(Duration::from_secs(60))
        .ok_or(IndexError::ResourceLimited)?;
    let mut work = WorkBudget::new(
        WorkLimits {
            reads: 100_000,
            writes: 100_000,
            bytes: 100_000_000,
            mutations: 100_000,
        },
        deadline,
    );
    let memory = Arc::new(TokenMemory {
        ceiling: 100_000_000,
        ..TokenMemory::default()
    });
    work.bind_memory_scope(Some(quanta_index_core::index::MemoryScope::new(Arc::new(
        BatchOutputMemoryScope(Arc::clone(&memory)),
    ))));
    let scratch = TestScratch::default();
    let spool = TextSpool::new(&scratch, &mut work)?;
    let custody = TestCustody;
    let maps = TestMaps::default();
    let objects = CountedOccurrences::default();
    let fixture = visitor_fixture()?;
    let attempt = test_attempt(fixture.ticket.view.identity.workspace);
    let mut confirmed = ConfirmedOccurrences::new(reuse_limit)?;
    let body = (0..65)
        .map(|index| format!("term{index:03}"))
        .collect::<Vec<_>>()
        .join(" ");
    let mut first_owner_prepare_reads = None;
    for ordinal in 0_u32..2 {
        let mut edits = InventoryEdits::new(ordinal)?;
        let mut writes = OccurrenceWrites::new();
        let mut context = AttemptContext {
            attempt,
            objects: &objects,
            maps: &maps,
            custody: &custody,
            scratch: &scratch,
            budget: &mut work,
        };
        let reads_before = context.budget.usage().reads;
        let PreparedInventory {
            indexed_tokens,
            term_count,
            ..
        } = prepare_inventory(
            &body,
            &EntryMap::default(),
            ordinal,
            &mut context,
            &spool,
            &mut confirmed,
            &mut edits,
            &mut writes,
        )?;
        check_eq!(indexed_tokens, 65);
        check_eq!(term_count, 130);
        if ordinal == 0 {
            // Both surfaces repeat the same 65 leaves before the first put.
            // Only the first appearance can need a negative confirmed probe.
            check_eq!(writes.pending.len(), 65);
            check_eq!(objects.physical_inputs.load(Ordering::Relaxed), 0);
            first_owner_prepare_reads = Some(
                context
                    .budget
                    .usage()
                    .reads
                    .checked_sub(reads_before)
                    .ok_or(IndexError::ResourceLimited)?,
            );
        }
        writes.flush(&mut context, &spool, &mut confirmed, &mut edits)?;
        edits.flush(&spool, context.budget)?;
    }
    let mut inventory = Vec::new();
    let mut cursor = spool.inventory_cursor(2)?;
    while let Some(row) = cursor.next_edit(&mut work)? {
        inventory.push((row.root, row.edit.key.clone(), row.edit.next));
    }
    drop(cursor);
    drop(confirmed);
    drop(spool);
    check_eq!(scratch.allocations.load(Ordering::Relaxed), 1);
    check_eq!(memory.live.load(Ordering::Relaxed), 0);
    check!(!memory.invalid_release.load(Ordering::Relaxed));
    let physical_inputs = objects.physical_inputs.load(Ordering::Relaxed);
    let per_identity = objects
        .per_identity
        .lock()
        .map_err(|_error| IndexError::Conflict)?
        .values()
        .copied()
        .collect();
    Ok((
        physical_inputs,
        per_identity,
        inventory,
        first_owner_prepare_reads.ok_or_else(|| {
            IndexError::BackendViolation("first owner prepare reads missing".into())
        })?,
    ))
}

#[test]
fn confirmed_spill_reuses_all_sixty_five_leaves_across_two_owners_without_changing_inventory()
-> Result<(), IndexError> {
    let (cached_puts, cached_counts, cached_inventory, cached_prepare_reads) =
        two_owner_inventory(64)?;
    let (uncached_puts, uncached_counts, uncached_inventory, uncached_prepare_reads) =
        two_owner_inventory(0)?;
    // The first 65 distinct leaves may probe the empty confirmed table. The
    // second 65 logical rows are aliases still pending in the same packet.
    check_eq!(
        cached_prepare_reads.checked_sub(uncached_prepare_reads),
        Some(65)
    );
    check_eq!(cached_puts, 65);
    // Raw and folded surfaces repeat each position 65 rows apart inside the
    // 256-object packet. Packet deduplication installs each once per owner;
    // confirmed spill additionally shares the 65 leaves across both owners.
    check_eq!(uncached_puts, 130);
    check_eq!(cached_counts.len(), 65);
    check!(cached_counts.iter().all(|count| *count == 1));
    check_eq!(uncached_counts.len(), 65);
    check!(uncached_counts.iter().all(|count| *count == 2));
    check_eq!(cached_inventory.len(), 260);
    check_eq!(cached_inventory, uncached_inventory);
    Ok(())
}

#[test]
fn refused_partial_or_malformed_install_cannot_publish_a_confirmed_prefix() -> Result<(), IndexError>
{
    #[derive(Serialize)]
    struct FixedOccurrence {
        positions: [u32; 1],
    }
    for case in 0..3 {
        let mut fixture = visitor_fixture()?;
        fixture.objects.install_outputs = match case {
            0 => None,
            1 => Some(vec![reference(90)]),
            _ => Some(vec![reference(90), reference(99)]),
        };
        let scratch = TestScratch::default();
        let custody = TestCustody;
        let mut setup = budget()?;
        let spool = TextSpool::new(&scratch, &mut setup)?;
        let mut writes = OccurrenceWrites::new();
        let mut inventory_edits = InventoryEdits::new(0)?;
        for (key, candidate) in [
            (b"\0a".as_slice(), reference(90)),
            (b"\x01a", reference(91)),
        ] {
            let payload =
                crate::native::text_memory::encode(&FixedOccurrence { positions: [0] }, &setup)?;
            writes.push(
                OccurrenceEdit {
                    ordinal: 0,
                    key: PreparedKey::copy(key, &setup)?,
                    prior: None,
                    identified: candidate,
                },
                payload,
                &spool,
                &mut inventory_edits,
                &mut setup,
            )?;
        }
        let (mut work, memory) = funded_batch_budget(1_000_000)?;
        let mut confirmed = ConfirmedOccurrences::new(64)?;
        let attempt = test_attempt(fixture.ticket.view.identity.workspace);
        let mut context = AttemptContext {
            attempt,
            objects: &fixture.objects,
            maps: &fixture.maps,
            custody: &custody,
            scratch: &scratch,
            budget: &mut work,
        };
        let result = writes.flush(&mut context, &spool, &mut confirmed, &mut inventory_edits);
        check!(result.is_err());
        check_eq!(fixture.objects.install_batches.load(Ordering::Relaxed), 1);
        for candidate in [reference(90), reference(91)] {
            check!(!spool.confirmed_occurrence_contains(candidate, attempt, context.budget)?);
        }
        check!(
            spool
                .inventory_cursor(1)?
                .next_edit(context.budget)
                .is_err()
        );
        drop(inventory_edits);
        drop(confirmed);
        drop(writes);
        check_eq!(
            memory.live.load(Ordering::Relaxed),
            inventory_packet_grant_bytes()
        );
        drop(spool);
        check_eq!(memory.live.load(Ordering::Relaxed), 0);
        check!(!memory.invalid_release.load(Ordering::Relaxed));
    }
    Ok(())
}
