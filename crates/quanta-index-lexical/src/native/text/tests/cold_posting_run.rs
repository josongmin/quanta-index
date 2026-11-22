//! Product-path oracle for a cold owner window that spans posting packet pages.

use super::super::entry_map::SmallMap;
use super::super::{cold_inventory_cursor::ColdInventoryProvenance, cold_posting_run::OwnerInput};
use super::*;
use quanta_index_core::index::{
    MemoryCredit, MemoryCreditLimits, MemoryLease, MemoryLifetime, MemoryScopePort,
};

struct FirstCreditOnly {
    inner: super::token_spool_batch::CreditOutputMemoryScope,
    requests: Arc<AtomicUsize>,
}

impl MemoryScopePort for FirstCreditOnly {
    fn reserve(&self, bytes: u64, lifetime: MemoryLifetime) -> Result<MemoryLease, IndexError> {
        self.inner.reserve(bytes, lifetime)
    }

    fn try_reserve_credit(
        &self,
        limits: MemoryCreditLimits,
    ) -> Result<Option<MemoryCredit>, IndexError> {
        if self.requests.fetch_add(1, Ordering::Relaxed) == 0 {
            self.inner.try_reserve_credit(limits)
        } else {
            Ok(None)
        }
    }
}

const INLINE_OWNERS: usize = 60;
const OWNERS: usize = INLINE_OWNERS + 2;
const TERMS: usize = 30;

fn fixed_keys() -> Vec<Vec<u8>> {
    (0..TERMS)
        .map(|ordinal| {
            let mut key = vec![0];
            key.extend_from_slice(format!("k{ordinal:02}").as_bytes());
            key
        })
        .collect()
}

fn fixed_block(keys: &[Vec<u8>]) -> Result<Vec<u8>, IndexError> {
    let mut wire = b"QITXOC01".to_vec();
    wire.extend_from_slice(
        &u16::try_from(keys.len())
            .map_err(|_error| IndexError::ResourceLimited)?
            .to_be_bytes(),
    );
    wire.extend_from_slice(&1_u16.to_be_bytes());
    wire.extend_from_slice(&0_u32.to_be_bytes());
    for key in keys {
        wire.extend_from_slice(
            &u16::try_from(key.len())
                .map_err(|_error| IndexError::ResourceLimited)?
                .to_be_bytes(),
        );
        wire.extend_from_slice(key);
        wire.extend_from_slice(&0_u16.to_be_bytes());
    }
    wire.extend_from_slice(&1_u16.to_be_bytes());
    wire.extend_from_slice(&0_u32.to_be_bytes());
    Ok(wire)
}

#[test]
fn unordered_sixty_four_owner_heads_keep_full_byte_order_across_packet_pages()
-> Result<(), IndexError> {
    let fixture = visitor_fixture()?;
    let keys: Vec<Vec<u8>> = fixed_keys()
        .into_iter()
        .map(|mut key| {
            key.resize(40, b'x');
            key
        })
        .collect();
    let payload = fixed_block(&keys)?;
    let registration =
        super::occurrence_blocks::CanonicalObjects::registration(&payload, Vec::new())?;
    let block = registration.reference;
    let mut objects = TestObjects::default();
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
    let mut work = WorkBudget::new(
        WorkLimits {
            reads: 100_000,
            writes: 100_000,
            bytes: 100_000_000,
            mutations: 100_000,
        },
        Instant::now()
            .checked_add(Duration::from_secs(120))
            .ok_or(IndexError::ResourceLimited)?,
    );
    work.bind_memory_scope(Some(quanta_index_core::index::MemoryScope::new(Arc::new(
        super::token_spool_batch::CreditOutputMemoryScope {
            memory: Arc::clone(&memory),
            attempts: Arc::new(AtomicUsize::new(0)),
        },
    ))));
    let scratch = TestScratch::default();
    let spool = TextSpool::new_cold_postings(&scratch, &mut work)?;
    check!(spool.prepare_ordered_cold_run_parent(&mut work)?);
    let run = spool
        .begin_ordered_cold_run(&mut work)?
        .ok_or(IndexError::ResourceLimited)?;
    let attempt = test_attempt(fixture.ticket.view.identity.workspace);
    let mut owners = Vec::new();
    let mut source_owners = Vec::new();
    for index in 0..64_u8 {
        let id = index.wrapping_mul(37) % 64;
        let mut owner = [0xa5; 32];
        owner[15] = id % 8;
        owner[31] = 63 - id;
        source_owners.push(owner);
        owners.push(OwnerInput {
            owner,
            provenance: ColdInventoryProvenance::Unsealed,
            next: Contribution {
                record: reference(id + 1),
                inventory: EntryMap::OccurrenceBlock(block),
                indexed_tokens: u64::try_from(TERMS)
                    .map_err(|_error| IndexError::ResourceLimited)?,
            },
            alias: None,
            directory_receipt: None,
        });
    }
    // Build the expected full tuple stream from the input fixture, without
    // using the tournament's ranks, LCP metadata, or emitted packet order.
    let mut expected: Vec<_> = keys
        .iter()
        .flat_map(|term| {
            source_owners
                .iter()
                .map(move |owner| (term.clone(), *owner, block))
        })
        .collect();
    expected.sort_by(|left, right| left.0.cmp(&right.0).then(left.1.cmp(&right.1)));
    let mut edits = PostingEdits::new()?;
    let mut context = AttemptContext {
        attempt,
        objects: &objects,
        maps: &fixture.maps,
        custody: &TestCustody,
        scratch: &scratch,
        budget: &mut work,
    };
    super::super::cold_posting_run::run_window(&owners, &mut context, &spool, &mut edits)?;
    run.finish(context.budget)?;
    let layout = spool.posting_packet_layout(context.budget)?;
    check!(
        layout
            .iter()
            .any(|(phase, run, page)| *phase == 0 && *run == 0 && *page > 0)
    );
    let terms = spool.posting_term_page(None, context.budget)?;
    check_eq!(terms.len(), TERMS);
    let mut cursor = spool.posting_cursor(terms);
    for (term, owner, reference) in &expected {
        let edit = cursor.next_edit(context.budget)?.ok_or_else(|| {
            IndexError::BackendViolation("unordered owner posting missing".into())
        })?;
        let ordinal = keys.binary_search(term).map_err(|_error| {
            IndexError::BackendViolation("oracle term missing from input".into())
        })?;
        check_eq!(
            edit.root(),
            u32::try_from(ordinal).map_err(|_error| IndexError::ResourceLimited)?
        );
        check_eq!(edit.key(), owner);
        check_eq!(edit.expected()?, None);
        check_eq!(edit.next()?, Some((*reference).into()));
    }
    check!(cursor.next_edit(context.budget)?.is_none());
    drop(cursor);
    drop(context);
    drop(edits);
    drop(spool);
    check_eq!(memory.live.load(Ordering::Relaxed), 0);
    check!(!memory.invalid_release.load(Ordering::Relaxed));
    Ok(())
}

#[test]
fn duplicate_term_owner_in_cold_merge_poisoned_before_publication() -> Result<(), IndexError> {
    let fixture = visitor_fixture()?;
    let keys = vec![b"\0a".to_vec(), b"\0b".to_vec()];
    let payload = fixed_block(&keys)?;
    let registration =
        super::occurrence_blocks::CanonicalObjects::registration(&payload, Vec::new())?;
    let block = registration.reference;
    let mut objects = TestObjects::default();
    check!(
        objects
            .stored
            .insert(block, StoredObject::new(registration, payload, None))
            .is_none()
    );
    let memory = Arc::new(TokenMemory {
        ceiling: 64 * 1024 * 1024,
        ..TokenMemory::default()
    });
    let mut work = WorkBudget::new(
        WorkLimits {
            reads: 10_000,
            writes: 10_000,
            bytes: 10_000_000,
            mutations: 10_000,
        },
        Instant::now()
            .checked_add(Duration::from_secs(60))
            .ok_or(IndexError::ResourceLimited)?,
    );
    work.bind_memory_scope(Some(quanta_index_core::index::MemoryScope::new(Arc::new(
        super::token_spool_batch::CreditOutputMemoryScope {
            memory: Arc::clone(&memory),
            attempts: Arc::new(AtomicUsize::new(0)),
        },
    ))));
    let scratch = TestScratch::default();
    let spool = TextSpool::new_cold_postings(&scratch, &mut work)?;
    check!(spool.prepare_ordered_cold_run_parent(&mut work)?);
    let run = spool
        .begin_ordered_cold_run(&mut work)?
        .ok_or(IndexError::ResourceLimited)?;
    let attempt = test_attempt(fixture.ticket.view.identity.workspace);
    let owners: [OwnerInput; 2] = std::array::from_fn(|_| OwnerInput {
        owner: [7; 32],
        provenance: ColdInventoryProvenance::Unsealed,
        next: Contribution {
            record: reference(7),
            inventory: EntryMap::OccurrenceBlock(block),
            indexed_tokens: 2,
        },
        alias: None,
        directory_receipt: None,
    });
    let mut edits = PostingEdits::new()?;
    let mut context = AttemptContext {
        attempt,
        objects: &objects,
        maps: &fixture.maps,
        custody: &TestCustody,
        scratch: &scratch,
        budget: &mut work,
    };
    let duplicate_result =
        super::super::cold_posting_run::run_window(&owners, &mut context, &spool, &mut edits);
    check!(
        matches!(
            &duplicate_result,
            Err(IndexError::CorruptObject(message))
                if message.contains("ordered cold posting keys are unordered")
        ),
        "duplicate result: {duplicate_result:?}"
    );
    drop(run);
    check!(matches!(
        spool.posting_term_page(None, context.budget),
        Err(IndexError::BackendViolation(_))
    ));
    drop(context);
    drop(edits);
    drop(spool);
    check_eq!(memory.live.load(Ordering::Relaxed), 0);
    check!(!memory.invalid_release.load(Ordering::Relaxed));
    Ok(())
}

#[test]
fn late_owner_eof_count_mismatch_publishes_no_packet_and_poisons_run() -> Result<(), IndexError> {
    let fixture = visitor_fixture()?;
    let keys = vec![b"\0a".to_vec(), b"\0b".to_vec()];
    let payload = fixed_block(&keys)?;
    let registration =
        super::occurrence_blocks::CanonicalObjects::registration(&payload, Vec::new())?;
    let block = registration.reference;
    let mut objects = TestObjects::default();
    check!(
        objects
            .stored
            .insert(block, StoredObject::new(registration, payload, None))
            .is_none()
    );
    let memory = Arc::new(TokenMemory {
        ceiling: 64 * 1024 * 1024,
        ..TokenMemory::default()
    });
    let mut work = WorkBudget::new(
        WorkLimits {
            reads: 10_000,
            writes: 10_000,
            bytes: 10_000_000,
            mutations: 10_000,
        },
        Instant::now()
            .checked_add(Duration::from_secs(60))
            .ok_or(IndexError::ResourceLimited)?,
    );
    work.bind_memory_scope(Some(quanta_index_core::index::MemoryScope::new(Arc::new(
        super::token_spool_batch::CreditOutputMemoryScope {
            memory: Arc::clone(&memory),
            attempts: Arc::new(AtomicUsize::new(0)),
        },
    ))));
    let scratch = TestScratch::default();
    let spool = TextSpool::new_cold_postings(&scratch, &mut work)?;
    check!(spool.prepare_ordered_cold_run_parent(&mut work)?);
    let run = spool
        .begin_ordered_cold_run(&mut work)?
        .ok_or(IndexError::ResourceLimited)?;
    let owner = OwnerInput {
        owner: [7; 32],
        provenance: ColdInventoryProvenance::Unsealed,
        next: Contribution {
            record: reference(7),
            inventory: EntryMap::OccurrenceBlock(block),
            // The checked block has no indexed positions. EOF must reject this
            // declared total after both term rows have been visited.
            indexed_tokens: 1,
        },
        alias: None,
        directory_receipt: None,
    };
    let mut edits = PostingEdits::new()?;
    let mut context = AttemptContext {
        attempt: test_attempt(fixture.ticket.view.identity.workspace),
        objects: &objects,
        maps: &fixture.maps,
        custody: &TestCustody,
        scratch: &scratch,
        budget: &mut work,
    };
    let result = super::super::cold_posting_run::run_window(
        std::slice::from_ref(&owner),
        &mut context,
        &spool,
        &mut edits,
    );
    check!(matches!(
        &result,
        Err(IndexError::CorruptObject(message))
            if message.contains("token count differs from inventory")
    ));
    check!(spool.posting_packet_layout(context.budget)?.is_empty());
    drop(run);
    check!(matches!(
        spool.posting_term_page(None, context.budget),
        Err(IndexError::BackendViolation(_))
    ));
    check_eq!(context.budget.first_denial(), None);
    drop(context);
    drop(edits);
    drop(spool);
    check_eq!(memory.live.load(Ordering::Relaxed), 0);
    check!(!memory.invalid_release.load(Ordering::Relaxed));
    Ok(())
}

#[test]
fn late_eof_after_a_persisted_packet_cannot_seal_or_publish_the_run() -> Result<(), IndexError> {
    let fixture = visitor_fixture()?;
    let keys = fixed_keys();
    let payload = fixed_block(&keys)?;
    let registration =
        super::occurrence_blocks::CanonicalObjects::registration(&payload, Vec::new())?;
    let block = registration.reference;
    let mut objects = TestObjects::default();
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
    let mut work = WorkBudget::new(
        WorkLimits {
            reads: 100_000,
            writes: 100_000,
            bytes: 100_000_000,
            mutations: 100_000,
        },
        Instant::now()
            .checked_add(Duration::from_secs(120))
            .ok_or(IndexError::ResourceLimited)?,
    );
    work.bind_memory_scope(Some(quanta_index_core::index::MemoryScope::new(Arc::new(
        super::token_spool_batch::CreditOutputMemoryScope {
            memory: Arc::clone(&memory),
            attempts: Arc::new(AtomicUsize::new(0)),
        },
    ))));
    let scratch = TestScratch::default();
    let spool = TextSpool::new_cold_postings(&scratch, &mut work)?;
    check!(spool.prepare_ordered_cold_run_parent(&mut work)?);
    let run = spool
        .begin_ordered_cold_run(&mut work)?
        .ok_or(IndexError::ResourceLimited)?;
    let owners: Vec<OwnerInput> = (1..=64_u8)
        .map(|owner| OwnerInput {
            owner: [owner; 32],
            provenance: ColdInventoryProvenance::Unsealed,
            next: Contribution {
                record: reference(owner),
                inventory: EntryMap::OccurrenceBlock(block),
                indexed_tokens: u64::from(owner == 64),
            },
            alias: None,
            directory_receipt: None,
        })
        .collect();
    let mut edits = PostingEdits::new()?;
    let mut context = AttemptContext {
        attempt: test_attempt(fixture.ticket.view.identity.workspace),
        objects: &objects,
        maps: &fixture.maps,
        custody: &TestCustody,
        scratch: &scratch,
        budget: &mut work,
    };
    let result =
        super::super::cold_posting_run::run_window(&owners, &mut context, &spool, &mut edits);
    check!(matches!(
        &result,
        Err(IndexError::CorruptObject(message))
            if message.contains("token count differs from inventory")
    ));
    // The packet page is a disposable effect. EOF refusal leaves no run seal.
    let layout = spool.posting_packet_layout(context.budget)?;
    check!(
        layout
            .iter()
            .any(|(phase, run, page)| *phase == 0 && *run == 0 && *page == 0)
    );
    drop(run);
    check!(matches!(
        spool.posting_term_page(None, context.budget),
        Err(IndexError::BackendViolation(_))
    ));
    check_eq!(context.budget.first_denial(), None);
    drop(context);
    drop(edits);
    drop(spool);
    check_eq!(memory.live.load(Ordering::Relaxed), 0);
    check!(!memory.invalid_release.load(Ordering::Relaxed));
    Ok(())
}

#[test]
fn two_prepared_small_owners_merge_in_one_ordered_run_and_sql_reopens_fixed_membership()
-> Result<(), IndexError> {
    let fixture = visitor_fixture()?;
    let keys = vec![b"\0a".to_vec(), b"\0b".to_vec()];
    let payload = fixed_block(&keys)?;
    let registration =
        super::occurrence_blocks::CanonicalObjects::registration(&payload, Vec::new())?;
    let block = registration.reference;
    let mut objects = TestObjects::default();
    check!(
        objects
            .stored
            .insert(block, StoredObject::new(registration, payload, None))
            .is_none()
    );
    let memory = Arc::new(TokenMemory {
        ceiling: 64 * 1024 * 1024,
        ..TokenMemory::default()
    });
    let mut work = WorkBudget::new(
        WorkLimits {
            reads: 10_000,
            writes: 10_000,
            bytes: 10_000_000,
            mutations: 10_000,
        },
        Instant::now()
            .checked_add(Duration::from_secs(60))
            .ok_or(IndexError::ResourceLimited)?,
    );
    let attempts = Arc::new(AtomicUsize::new(0));
    work.bind_memory_scope(Some(quanta_index_core::index::MemoryScope::new(Arc::new(
        super::token_spool_batch::CreditOutputMemoryScope {
            memory: Arc::clone(&memory),
            attempts: Arc::clone(&attempts),
        },
    ))));
    let scratch = TestScratch::default();
    let spool = TextSpool::new_cold_postings(&scratch, &mut work)?;
    let attempt = test_attempt(fixture.ticket.view.identity.workspace);
    let mut batch = InventoryBatch::cold(2)?;
    batch.occurrence_writes.pack_owners = true;
    batch.attempt = Some(attempt);
    for ordinal in 0..2usize {
        let owner = u8::try_from(ordinal + 1).map_err(|_error| IndexError::ResourceLimited)?;
        batch.rows.push(
            PendingTextChange {
                owner: [owner; 32],
                prior_ref: None,
                prior: None,
                next: PendingTextNext::Draft {
                    record: reference(owner),
                    slot: ordinal,
                    body: TextBodyKey {
                        bytes: reference(owner),
                        byte_len: 0,
                    },
                    tokens: 2,
                    alias: None,
                    shared: false,
                },
            },
            &work,
        )?;
        batch.ready.push(ReadyTextNext::default(), &work)?;
        batch.bases.push(EntryMap::default(), &work)?;
        batch.counts.push(2, &work)?;
        batch.directory_receipts.push(None, &work)?;
        for key in &keys {
            batch.inventory_edits.push_partition(
                InventoryPartition::Ready(
                    u32::try_from(ordinal).map_err(|_error| IndexError::ResourceLimited)?,
                ),
                PreparedKey::copy(key, &work)?,
                None,
                Some(block.into()),
                &work,
            )?;
        }
    }
    let mut root = Root::default();
    let mut contributions = ContributionWrites::cold()?;
    let mut inventories = InventoryReuse::new(fixture.recipe, 64)?;
    let mut context = AttemptContext {
        attempt,
        objects: &objects,
        maps: &fixture.maps,
        custody: &TestCustody,
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
    check_eq!((root.docs, root.indexed_tokens), (2, 4));
    check!(attempts.load(Ordering::Relaxed) > 0);
    check_eq!(
        spool.posting_packet_layout(context.budget)?,
        vec![(0, 0, 0)]
    );
    let terms = spool.posting_term_page(None, context.budget)?;
    check_eq!(terms.len(), 2);
    let mut cursor = spool.posting_cursor(terms);
    for root_ordinal in 0..2u32 {
        for owner in 1..=2u8 {
            let edit = cursor.next_edit(context.budget)?.ok_or_else(|| {
                IndexError::BackendViolation("prepared small posting missing".into())
            })?;
            check_eq!(edit.root(), root_ordinal);
            check_eq!(edit.key(), &[owner; 32]);
            check_eq!(edit.expected()?, None);
            check_eq!(edit.next()?, Some(block.into()));
        }
    }
    check!(cursor.next_edit(context.budget)?.is_none());
    drop(cursor);
    drop(context);
    drop(batch);
    drop(contributions);
    drop(inventories);
    drop(spool);
    check_eq!(memory.live.load(Ordering::Relaxed), 0);
    check!(!memory.invalid_release.load(Ordering::Relaxed));
    Ok(())
}

#[test]
fn cold_owner_window_produces_ordered_multipage_run_and_reopens_all_rows() -> Result<(), IndexError>
{
    let mut fixture = visitor_fixture()?;
    let keys = fixed_keys();
    let payload = fixed_block(&keys)?;
    let registration =
        super::occurrence_blocks::CanonicalObjects::registration(&payload, Vec::new())?;
    let block = registration.reference;
    let mut objects = TestObjects::default();
    let previous = objects
        .stored
        .insert(block, StoredObject::new(registration, payload, None));
    check!(previous.is_none());

    let memory = Arc::new(TokenMemory {
        ceiling: 512 * 1024 * 1024,
        ..TokenMemory::default()
    });
    let mut work = WorkBudget::new(
        WorkLimits {
            reads: 100_000,
            writes: 100_000,
            bytes: 100_000_000,
            mutations: 100_000,
        },
        Instant::now()
            .checked_add(Duration::from_secs(120))
            .ok_or(IndexError::ResourceLimited)?,
    );
    let credit_attempts = Arc::new(AtomicUsize::new(0));
    work.bind_memory_scope(Some(quanta_index_core::index::MemoryScope::new(Arc::new(
        super::token_spool_batch::CreditOutputMemoryScope {
            memory: Arc::clone(&memory),
            attempts: Arc::clone(&credit_attempts),
        },
    ))));
    let scratch = TestScratch::default();
    let spool = TextSpool::new_cold_postings(&scratch, &mut work)?;
    let attempt = test_attempt(fixture.ticket.view.identity.workspace);
    let mut seal = entry_patch::ColdInventoryKeySeal::new();
    for key in &keys {
        seal.push(key, &mut work)?;
    }
    let alias = entry_patch::ColdInventoryAlias {
        reference: block,
        terms: u64::try_from(TERMS).map_err(|_error| IndexError::ResourceLimited)?,
        key_digest: seal.finish(),
        anchors: 1,
        descriptor_digest: entry_patch::ColdDescriptorSeal::new().finish(),
        layout: entry_patch::ColdAliasLayout::Inline,
        attempt,
    };
    let directory = reference(242);
    fixture.maps.add(
        MapRoot(Some(directory)),
        keys.last().ok_or(IndexError::ResourceLimited)?.clone(),
        entry_map::directory::descriptor(1, TERMS, block)?,
    )?;
    let mut small = SmallMap::default();
    check!(small.push(&keys[0], block)?);
    let small = EntryMap::inline(&small);

    let mut batch = InventoryBatch::cold(OWNERS)?;
    batch.occurrence_writes.pack_owners = true;
    batch.attempt = Some(attempt);
    for ordinal in 0..OWNERS {
        let owner = u8::try_from(ordinal.checked_add(1).ok_or(IndexError::ResourceLimited)?)
            .map_err(|_error| IndexError::ResourceLimited)?;
        let (next, ready) = if ordinal < INLINE_OWNERS {
            (
                PendingTextNext::Draft {
                    record: reference(owner),
                    slot: ordinal,
                    body: TextBodyKey {
                        bytes: reference(owner),
                        byte_len: 0,
                    },
                    tokens: u64::try_from(TERMS).map_err(|_error| IndexError::ResourceLimited)?,
                    alias: Some(alias),
                    shared: false,
                },
                ReadyTextNext::default(),
            )
        } else {
            let (inventory, indexed_tokens) = if ordinal == INLINE_OWNERS {
                (
                    EntryMap::InventoryDirectory(directory),
                    u64::try_from(TERMS).map_err(|_error| IndexError::ResourceLimited)?,
                )
            } else {
                (small, 1)
            };
            (
                PendingTextNext::Ready,
                ReadyTextNext {
                    contribution: Some(Contribution {
                        record: reference(owner),
                        inventory,
                        indexed_tokens,
                    }),
                    reused: false,
                    verified: None,
                },
            )
        };
        batch.rows.push(
            PendingTextChange {
                owner: [owner; 32],
                prior_ref: None,
                prior: None,
                next,
            },
            &work,
        )?;
        batch.ready.push(ready, &work)?;
        if ordinal >= INLINE_OWNERS {
            continue;
        }
        batch.bases.push(EntryMap::default(), &work)?;
        batch.counts.push(1, &work)?;
        batch.directory_receipts.push(None, &work)?;
        batch.inventory_edits.push_partition(
            InventoryPartition::Ready(
                u32::try_from(ordinal).map_err(|_error| IndexError::ResourceLimited)?,
            ),
            PreparedKey::copy(&keys[0], &work)?,
            None,
            Some(block.into()),
            &work,
        )?;
    }

    let mut root = Root::default();
    let mut contributions = ContributionWrites::cold()?;
    let mut inventories = InventoryReuse::new(fixture.recipe, 64)?;
    let mut context = AttemptContext {
        attempt,
        objects: &objects,
        maps: &fixture.maps,
        custody: &TestCustody,
        scratch: &scratch,
        budget: &mut work,
    };
    let baseline_before = context.budget.usage().bytes;
    batch.flush(
        &mut root,
        &mut context,
        &spool,
        &mut contributions,
        &mut inventories,
    )?;
    check_eq!(
        root.docs,
        u64::try_from(OWNERS).map_err(|_error| IndexError::ResourceLimited)?
    );
    check_eq!(root.indexed_tokens, 1_831);
    let stage_bytes = context
        .budget
        .usage()
        .bytes
        .checked_sub(baseline_before)
        .ok_or(IndexError::ResourceLimited)?;
    let layout = spool.posting_packet_layout(context.budget)?;
    check!(
        layout
            .iter()
            .any(|(phase, run, page)| *phase == 0 && *run == 0 && *page == 0)
    );
    check!(
        layout
            .iter()
            .any(|(phase, run, page)| *phase == 0 && *run == 0 && *page > 0)
    );
    check!(credit_attempts.load(Ordering::Relaxed) > 0);

    // 60 inline aliases, one directory and one small inventory produce 1,831
    // postings, above the 1,772-row packet bound. The
    // reopened public cursor must still yield every tuple in term-major order.
    let terms = spool.posting_term_page(None, context.budget)?;
    check_eq!(terms.len(), TERMS);
    let mut cursor = spool.posting_cursor(terms);
    for (term, _key) in keys.iter().enumerate() {
        let owners_for_term = if term == 0 { OWNERS } else { INLINE_OWNERS + 1 };
        for owner in 1..=owners_for_term {
            let edit = cursor
                .next_edit(context.budget)?
                .ok_or_else(|| IndexError::BackendViolation("posting row missing".into()))?;
            check_eq!(
                edit.root(),
                u32::try_from(term).map_err(|_error| IndexError::ResourceLimited)?
            );
            check_eq!(
                edit.key(),
                &[u8::try_from(owner).map_err(|_error| IndexError::ResourceLimited)?; 32]
            );
            check_eq!(edit.expected()?, None);
            check_eq!(edit.next()?, Some(block.into()));
        }
    }
    check!(cursor.next_edit(context.budget)?.is_none());
    drop(cursor);
    drop(context);
    drop(batch);
    drop(contributions);
    drop(inventories);
    drop(spool);
    check_eq!(memory.live.load(Ordering::Relaxed), 0);
    check!(!memory.invalid_release.load(Ordering::Relaxed));
    // The prior borrowed-head winner charged 707,886B for this same stage.
    // Exact LCP replay must improve it while retaining the full tuple oracle.
    check!(stage_bytes < 707_886);
    check_eq!(stage_bytes, 695_905);
    Ok(())
}

#[test]
fn ready_packed_owner_uses_bounded_ordered_run_and_reopens_eof() -> Result<(), IndexError> {
    ready_owner_runs(false)
}

#[test]
fn ready_directory_fallback_keeps_later_owners_on_ordered_run() -> Result<(), IndexError> {
    ready_owner_runs(true)
}

fn ready_owner_runs(directory_gap: bool) -> Result<(), IndexError> {
    let mut fixture = visitor_fixture()?;
    let keys = fixed_keys();
    let payload = fixed_block(&keys)?;
    let registration =
        super::occurrence_blocks::CanonicalObjects::registration(&payload, Vec::new())?;
    let block = registration.reference;
    let mut objects = TestObjects::default();
    let previous = objects
        .stored
        .insert(block, StoredObject::new(registration, payload, None));
    check!(previous.is_none());

    let memory = Arc::new(TokenMemory {
        ceiling: 64 * 1024 * 1024,
        ..TokenMemory::default()
    });
    let mut work = WorkBudget::new(
        WorkLimits {
            reads: 10_000,
            writes: 10_000,
            bytes: 10_000_000,
            mutations: 10_000,
        },
        Instant::now()
            .checked_add(Duration::from_secs(60))
            .ok_or(IndexError::ResourceLimited)?,
    );
    let attempts = Arc::new(AtomicUsize::new(0));
    work.bind_memory_scope(Some(quanta_index_core::index::MemoryScope::new(Arc::new(
        super::token_spool_batch::CreditOutputMemoryScope {
            memory: Arc::clone(&memory),
            attempts: Arc::clone(&attempts),
        },
    ))));
    let scratch = TestScratch::default();
    let spool = TextSpool::new_cold_postings(&scratch, &mut work)?;
    let attempt = test_attempt(fixture.ticket.view.identity.workspace);
    let owners = if directory_gap { 4_u8 } else { 1_u8 };
    let directory = MapRoot(Some(reference(242)));
    if directory_gap {
        fixture.maps.add(
            directory,
            keys.last().ok_or(IndexError::Conflict)?.clone(),
            entry_map::directory::descriptor(1, TERMS, block)?,
        )?;
    }
    let indexed_tokens = u64::try_from(TERMS).map_err(|_error| IndexError::ResourceLimited)?;
    let mut batch = InventoryBatch::cold(usize::from(owners))?;
    batch.occurrence_writes.pack_owners = true;
    batch.attempt = Some(attempt);
    for owner in 1..=owners {
        batch.rows.push(
            PendingTextChange {
                owner: [owner; 32],
                prior_ref: None,
                prior: None,
                next: PendingTextNext::Ready,
            },
            &work,
        )?;
        let inventory = if (2..=3).contains(&owner) {
            EntryMap::InventoryDirectory(directory.0.ok_or(IndexError::Conflict)?)
        } else {
            EntryMap::OccurrenceBlock(block)
        };
        batch.ready.push(
            ReadyTextNext {
                contribution: Some(Contribution {
                    record: reference(owner),
                    inventory,
                    indexed_tokens,
                }),
                reused: false,
                verified: None,
            },
            &work,
        )?;
    }
    let mut root = Root::default();
    let mut contributions = ContributionWrites::cold()?;
    let mut inventories = InventoryReuse::new(fixture.recipe, 64)?;
    let mut context = AttemptContext {
        attempt,
        objects: &objects,
        maps: &fixture.maps,
        custody: &TestCustody,
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
    check_eq!(root.docs, u64::from(owners));
    check_eq!(
        Some(root.indexed_tokens),
        indexed_tokens.checked_mul(u64::from(owners))
    );
    check!(attempts.load(Ordering::Relaxed) >= if directory_gap { 2 } else { 1 });
    check_eq!(
        spool.posting_packet_layout(context.budget)?,
        if directory_gap {
            vec![(0, 0, 0), (0, 1, 0), (0, 2, 0)]
        } else {
            vec![(0, 0, 0)]
        }
    );
    let terms = spool.posting_term_page(None, context.budget)?;
    check_eq!(terms.len(), TERMS);
    let mut cursor = spool.posting_cursor(terms);
    for ordinal in 0..TERMS {
        for owner in 1..=owners {
            let edit = cursor.next_edit(context.budget)?.ok_or_else(|| {
                IndexError::BackendViolation("packed owner posting missing".into())
            })?;
            check_eq!(
                edit.root(),
                u32::try_from(ordinal).map_err(|_error| IndexError::ResourceLimited)?
            );
            check_eq!(edit.key(), &[owner; 32]);
            check_eq!(edit.expected()?, None);
            check_eq!(edit.next()?, Some(block.into()));
        }
    }
    check!(cursor.next_edit(context.budget)?.is_none());
    drop(cursor);
    drop(context);
    drop(batch);
    drop(contributions);
    drop(inventories);
    drop(spool);
    check_eq!(memory.live.load(Ordering::Relaxed), 0);
    check!(!memory.invalid_release.load(Ordering::Relaxed));
    Ok(())
}

#[test]
fn wide_posting_writer_funds_long_keys_through_full_packet_and_eof() -> Result<(), IndexError> {
    const ROWS: usize = quanta_index_scratch_sqlite::MAX_WIDE_WRITE_BATCH_ROWS;
    let fixture = visitor_fixture()?;
    let keys: Vec<Vec<u8>> = (0..ROWS)
        .map(|ordinal| {
            let mut key = format!("\0k{ordinal:03}").into_bytes();
            key.resize(40, b'x');
            key
        })
        .collect();
    let payload = fixed_block(&keys)?;
    check!(payload.len() <= 8 * 1024);
    let registration =
        super::occurrence_blocks::CanonicalObjects::registration(&payload, Vec::new())?;
    let block = registration.reference;
    let mut objects = TestObjects::default();
    check!(
        objects
            .stored
            .insert(block, StoredObject::new(registration, payload, None))
            .is_none()
    );

    let memory = Arc::new(TokenMemory {
        ceiling: 64 * 1024 * 1024,
        ..TokenMemory::default()
    });
    let attempts = Arc::new(AtomicUsize::new(0));
    let mut work = WorkBudget::new(
        WorkLimits {
            reads: 10_000,
            writes: 10_000,
            bytes: 10_000_000,
            mutations: 10_000,
        },
        Instant::now()
            .checked_add(Duration::from_secs(60))
            .ok_or(IndexError::ResourceLimited)?,
    );
    work.bind_memory_scope(Some(quanta_index_core::index::MemoryScope::new(Arc::new(
        super::token_spool_batch::CreditOutputMemoryScope {
            memory: Arc::clone(&memory),
            attempts: Arc::clone(&attempts),
        },
    ))));
    let scratch = TestScratch::default();
    let spool = TextSpool::new_cold_postings(&scratch, &mut work)?;
    let posting_width = spool.posting_write_batch_rows(&work)?;
    check_eq!(posting_width, ROWS);
    let attempt = test_attempt(fixture.ticket.view.identity.workspace);
    let mut batch = InventoryBatch::cold(1)?;
    batch.posting_edits = PostingEdits::with_limit(posting_width)?;
    batch.occurrence_writes.pack_owners = true;
    batch.attempt = Some(attempt);
    batch.rows.push(
        PendingTextChange {
            owner: [1; 32],
            prior_ref: None,
            prior: None,
            next: PendingTextNext::Ready,
        },
        &work,
    )?;
    batch.ready.push(
        ReadyTextNext {
            contribution: Some(Contribution {
                record: reference(1),
                inventory: EntryMap::OccurrenceBlock(block),
                indexed_tokens: u64::try_from(ROWS)
                    .map_err(|_error| IndexError::ResourceLimited)?,
            }),
            reused: false,
            verified: None,
        },
        &work,
    )?;
    let mut root = Root::default();
    let mut contributions = ContributionWrites::cold()?;
    let mut inventories = InventoryReuse::new(fixture.recipe, 64)?;
    let mut context = AttemptContext {
        attempt,
        objects: &objects,
        maps: &fixture.maps,
        custody: &TestCustody,
        scratch: &scratch,
        budget: &mut work,
    };
    if let Err(error) = batch.flush(
        &mut root,
        &mut context,
        &spool,
        &mut contributions,
        &mut inventories,
    ) {
        return Err(IndexError::BackendViolation(format!(
            "wide posting flush failed: {error:?}; first_denial={:?}",
            context.budget.first_denial()
        )));
    }
    check_eq!(
        (root.docs, root.indexed_tokens),
        (
            1,
            u64::try_from(ROWS).map_err(|_error| IndexError::ResourceLimited)?,
        )
    );
    check!(attempts.load(Ordering::Relaxed) > 0);
    check_eq!(context.budget.first_denial(), None);
    let mut observed = 0_usize;
    while observed < ROWS {
        let after = observed
            .checked_sub(1)
            .and_then(|prior| keys.get(prior))
            .map(Vec::as_slice);
        let terms = spool.posting_term_page(after, context.budget)?;
        let page_len = terms.len();
        check!(page_len > 0 && page_len <= quanta_index_scratch_sqlite::MAX_WRITE_BATCH_ROWS);
        for (offset, term) in terms.iter().enumerate() {
            let expected = keys
                .get(
                    observed
                        .checked_add(offset)
                        .ok_or(IndexError::ResourceLimited)?,
                )
                .ok_or(IndexError::ResourceLimited)?;
            check_eq!(term.as_slice(), expected.as_slice());
        }
        let mut cursor = spool.posting_cursor(terms);
        for ordinal in 0..page_len {
            let edit = cursor
                .next_edit(context.budget)?
                .ok_or_else(|| IndexError::BackendViolation("wide posting row missing".into()))?;
            check_eq!(
                edit.root(),
                u32::try_from(ordinal).map_err(|_error| IndexError::ResourceLimited)?
            );
            check_eq!(edit.key(), &[1; 32]);
            check_eq!(edit.expected()?, None);
            check_eq!(edit.next()?, Some(block.into()));
        }
        check!(cursor.next_edit(context.budget)?.is_none());
        observed = observed
            .checked_add(page_len)
            .ok_or(IndexError::ResourceLimited)?;
    }
    check_eq!(observed, ROWS);
    check!(
        spool
            .posting_term_page(keys.last().map(Vec::as_slice), context.budget)?
            .is_empty()
    );
    check_eq!(context.budget.first_denial(), None);
    drop(context);
    drop(batch);
    drop(contributions);
    drop(inventories);
    drop(spool);
    check_eq!(memory.live.load(Ordering::Relaxed), 0);
    check!(!memory.invalid_release.load(Ordering::Relaxed));
    Ok(())
}

#[test]
fn later_credit_miss_replays_no_prefix_and_uses_legacy_unread_suffix() -> Result<(), IndexError> {
    const OWNERS: usize = 65;
    let fixture = visitor_fixture()?;
    let keys = fixed_keys();
    let payload = fixed_block(&keys)?;
    let registration =
        super::occurrence_blocks::CanonicalObjects::registration(&payload, Vec::new())?;
    let block = registration.reference;
    let mut objects = TestObjects::default();
    let previous = objects
        .stored
        .insert(block, StoredObject::new(registration, payload, None));
    check!(previous.is_none());

    let memory = Arc::new(TokenMemory {
        ceiling: 512 * 1024 * 1024,
        ..TokenMemory::default()
    });
    let requests = Arc::new(AtomicUsize::new(0));
    let mut work = WorkBudget::new(
        WorkLimits {
            reads: 100_000,
            writes: 100_000,
            bytes: 100_000_000,
            mutations: 100_000,
        },
        Instant::now()
            .checked_add(Duration::from_secs(120))
            .ok_or(IndexError::ResourceLimited)?,
    );
    work.bind_memory_scope(Some(quanta_index_core::index::MemoryScope::new(Arc::new(
        FirstCreditOnly {
            inner: super::token_spool_batch::CreditOutputMemoryScope {
                memory: Arc::clone(&memory),
                attempts: Arc::new(AtomicUsize::new(0)),
            },
            requests: Arc::clone(&requests),
        },
    ))));
    let scratch = TestScratch::default();
    let spool = TextSpool::new_cold_postings(&scratch, &mut work)?;
    let attempt = test_attempt(fixture.ticket.view.identity.workspace);
    let mut batch = InventoryBatch::cold(OWNERS)?;
    batch.occurrence_writes.pack_owners = true;
    batch.attempt = Some(attempt);
    for ordinal in 0..OWNERS {
        let owner = u8::try_from(ordinal.checked_add(1).ok_or(IndexError::ResourceLimited)?)
            .map_err(|_error| IndexError::ResourceLimited)?;
        batch.rows.push(
            PendingTextChange {
                owner: [owner; 32],
                prior_ref: None,
                prior: None,
                next: PendingTextNext::Ready,
            },
            &work,
        )?;
        batch.ready.push(
            ReadyTextNext {
                contribution: Some(Contribution {
                    record: reference(owner),
                    inventory: EntryMap::OccurrenceBlock(block),
                    indexed_tokens: u64::try_from(TERMS)
                        .map_err(|_error| IndexError::ResourceLimited)?,
                }),
                reused: false,
                verified: None,
            },
            &work,
        )?;
    }
    let mut root = Root::default();
    let mut contributions = ContributionWrites::cold()?;
    let mut inventories = InventoryReuse::new(fixture.recipe, 64)?;
    let mut context = AttemptContext {
        attempt,
        objects: &objects,
        maps: &fixture.maps,
        custody: &TestCustody,
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
    check_eq!(root.docs, 65);
    check_eq!(root.indexed_tokens, 1_950);
    check!(requests.load(Ordering::Relaxed) >= 2);
    check_eq!(context.budget.first_denial(), None);
    let before_seal = spool.posting_packet_layout(context.budget)?;
    check!(
        before_seal
            .iter()
            .any(|(phase, run, page)| *phase == 0 && *run == 0 && *page > 0)
    );
    let terms = spool.posting_term_page(None, context.budget)?;
    check_eq!(terms.len(), TERMS);
    let after_seal = spool.posting_packet_layout(context.budget)?;
    check!(
        after_seal
            .iter()
            .any(|(phase, run, page)| *phase == 0 && *run == 1 && *page == 0)
    );
    let mut cursor = spool.posting_cursor(terms);
    for ordinal in 0..TERMS {
        for owner in 1..=OWNERS {
            let edit = cursor
                .next_edit(context.budget)?
                .ok_or_else(|| IndexError::BackendViolation("fallback posting missing".into()))?;
            check_eq!(
                edit.root(),
                u32::try_from(ordinal).map_err(|_error| IndexError::ResourceLimited)?
            );
            check_eq!(
                edit.key(),
                &[u8::try_from(owner).map_err(|_error| IndexError::ResourceLimited)?; 32]
            );
            check_eq!(edit.expected()?, None);
            check_eq!(edit.next()?, Some(block.into()));
        }
    }
    check!(cursor.next_edit(context.budget)?.is_none());
    check_eq!(context.budget.first_denial(), None);
    drop(cursor);
    drop(context);
    drop(batch);
    drop(contributions);
    drop(inventories);
    drop(spool);
    check_eq!(memory.live.load(Ordering::Relaxed), 0);
    check!(!memory.invalid_release.load(Ordering::Relaxed));
    Ok(())
}

#[test]
fn long_distinct_heads_keep_full_order_and_accounted_comparison_work() -> Result<(), IndexError> {
    let fixture = visitor_fixture()?;
    let keys: Vec<Vec<u8>> = (0_u8..64)
        .map(|index| {
            let mut key = vec![0];
            key.extend_from_slice(format!("k{index:03}").as_bytes());
            key.resize(32, b'x');
            key
        })
        .collect();
    let payload = fixed_block(&keys)?;
    let registration =
        super::occurrence_blocks::CanonicalObjects::registration(&payload, Vec::new())?;
    let block = registration.reference;
    let mut objects = TestObjects::default();
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
    let mut work = WorkBudget::new(
        WorkLimits {
            reads: 100_000,
            writes: 100_000,
            bytes: 100_000_000,
            mutations: 100_000,
        },
        Instant::now()
            .checked_add(Duration::from_secs(120))
            .ok_or(IndexError::ResourceLimited)?,
    );
    work.bind_memory_scope(Some(quanta_index_core::index::MemoryScope::new(Arc::new(
        super::token_spool_batch::CreditOutputMemoryScope {
            memory: Arc::clone(&memory),
            attempts: Arc::new(AtomicUsize::new(0)),
        },
    ))));
    let scratch = TestScratch::default();
    let spool = TextSpool::new_cold_postings(&scratch, &mut work)?;
    check!(spool.prepare_ordered_cold_run_parent(&mut work)?);
    let run = spool
        .begin_ordered_cold_run(&mut work)?
        .ok_or(IndexError::ResourceLimited)?;
    let mut owners = Vec::new();
    let mut expected_owners = [[0_u8; 32]; 64];
    for index in 0_u8..64 {
        let mut small = SmallMap::default();
        check!(small.push(&keys[usize::from(index)], block)?);
        let owner = [index.wrapping_mul(37); 32];
        expected_owners[usize::from(index)] = owner;
        owners.push(OwnerInput {
            owner,
            provenance: ColdInventoryProvenance::PreparedCanonical,
            next: Contribution {
                record: reference(index.checked_add(1).ok_or(IndexError::ResourceLimited)?),
                inventory: EntryMap::inline(&small),
                indexed_tokens: 1,
            },
            alias: None,
            directory_receipt: None,
        });
    }
    let mut edits = PostingEdits::new()?;
    let mut context = AttemptContext {
        attempt: test_attempt(fixture.ticket.view.identity.workspace),
        objects: &objects,
        maps: &fixture.maps,
        custody: &TestCustody,
        scratch: &scratch,
        budget: &mut work,
    };
    let before = context.budget.usage().bytes;
    super::super::cold_posting_run::run_window(&owners, &mut context, &spool, &mut edits)?;
    run.finish(context.budget)?;
    let stage_bytes = context
        .budget
        .usage()
        .bytes
        .checked_sub(before)
        .ok_or(IndexError::ResourceLimited)?;
    let terms = spool.posting_term_page(None, context.budget)?;
    check_eq!(terms.len(), 64);
    let mut cursor = spool.posting_cursor(terms);
    for (ordinal, owner) in expected_owners.iter().enumerate() {
        let edit = cursor
            .next_edit(context.budget)?
            .ok_or(IndexError::ResourceLimited)?;
        check_eq!(
            edit.root(),
            u32::try_from(ordinal).map_err(|_| IndexError::ResourceLimited)?
        );
        check_eq!(edit.key(), owner);
        check_eq!(edit.expected()?, None);
        check_eq!(edit.next()?, Some(block.into()));
    }
    check!(cursor.next_edit(context.budget)?.is_none());
    drop(cursor);
    drop(context);
    drop(edits);
    drop(spool);
    check_eq!(memory.live.load(Ordering::Relaxed), 0);
    check!(!memory.invalid_release.load(Ordering::Relaxed));
    // The prior borrowed-head winner charged 56,748B for this same stage.
    check!(stage_bytes < 56_748);
    check_eq!(stage_bytes, 55_009);
    Ok(())
}

#[test]
fn winner_heads_replace_and_compare_equal_prefix_and_short_keys() -> Result<(), IndexError> {
    let fixture = visitor_fixture()?;
    let mixed = vec![
        vec![b"\0aaa".to_vec(), b"\0aab".to_vec(), b"\0ac".to_vec()],
        vec![b"\0ab".to_vec(), b"\0zz".to_vec()],
        vec![b"\0aaaz".to_vec(), b"\0ac".to_vec()],
    ];
    let mut objects = TestObjects::default();
    let mut owners = Vec::new();
    let mut expected = Vec::new();
    for (slot, keys) in mixed.iter().enumerate() {
        let payload = fixed_block(keys)?;
        let registration =
            super::occurrence_blocks::CanonicalObjects::registration(&payload, Vec::new())?;
        let block = registration.reference;
        check!(
            objects
                .stored
                .insert(block, StoredObject::new(registration, payload, None))
                .is_none()
        );
        let ordinal = slot.checked_add(1).ok_or(IndexError::ResourceLimited)?;
        let owner = [u8::try_from(ordinal).map_err(|_| IndexError::ResourceLimited)?; 32];
        owners.push(OwnerInput {
            owner,
            provenance: ColdInventoryProvenance::Unsealed,
            next: Contribution {
                record: reference(u8::try_from(ordinal).map_err(|_| IndexError::ResourceLimited)?),
                inventory: EntryMap::OccurrenceBlock(block),
                indexed_tokens: u64::try_from(keys.len())
                    .map_err(|_| IndexError::ResourceLimited)?,
            },
            alias: None,
            directory_receipt: None,
        });
        for key in keys {
            expected.push((key.clone(), owner, block));
        }
    }
    owners.push(OwnerInput {
        owner: [0; 32],
        provenance: ColdInventoryProvenance::PreparedCanonical,
        next: Contribution {
            record: reference(9),
            inventory: EntryMap::default(),
            indexed_tokens: 0,
        },
        alias: None,
        directory_receipt: None,
    });
    expected.sort_by(|left, right| left.0.cmp(&right.0).then(left.1.cmp(&right.1)));
    let mut distinct_terms: Vec<Vec<u8>> = expected.iter().map(|row| row.0.clone()).collect();
    distinct_terms.sort();
    distinct_terms.dedup();
    let memory = Arc::new(TokenMemory {
        ceiling: 512 * 1024 * 1024,
        ..TokenMemory::default()
    });
    let mut work = WorkBudget::new(
        WorkLimits {
            reads: 100_000,
            writes: 100_000,
            bytes: 100_000_000,
            mutations: 100_000,
        },
        Instant::now()
            .checked_add(Duration::from_secs(60))
            .ok_or(IndexError::ResourceLimited)?,
    );
    work.bind_memory_scope(Some(quanta_index_core::index::MemoryScope::new(Arc::new(
        super::token_spool_batch::CreditOutputMemoryScope {
            memory: Arc::clone(&memory),
            attempts: Arc::new(AtomicUsize::new(0)),
        },
    ))));
    let scratch = TestScratch::default();
    let spool = TextSpool::new_cold_postings(&scratch, &mut work)?;
    check!(spool.prepare_ordered_cold_run_parent(&mut work)?);
    let run = spool
        .begin_ordered_cold_run(&mut work)?
        .ok_or(IndexError::ResourceLimited)?;
    let mut edits = PostingEdits::new()?;
    let mut context = AttemptContext {
        attempt: test_attempt(fixture.ticket.view.identity.workspace),
        objects: &objects,
        maps: &fixture.maps,
        custody: &TestCustody,
        scratch: &scratch,
        budget: &mut work,
    };
    super::super::cold_posting_run::run_window(&owners, &mut context, &spool, &mut edits)?;
    run.finish(context.budget)?;
    let terms = spool.posting_term_page(None, context.budget)?;
    check_eq!(terms.len(), distinct_terms.len());
    let mut cursor = spool.posting_cursor(terms);
    for (term, owner, block) in &expected {
        let edit = cursor
            .next_edit(context.budget)?
            .ok_or(IndexError::ResourceLimited)?;
        let expected_root = u32::try_from(
            distinct_terms
                .binary_search(term)
                .map_err(|_| IndexError::ResourceLimited)?,
        )
        .map_err(|_| IndexError::ResourceLimited)?;
        check_eq!(edit.root(), expected_root);
        check_eq!(edit.key(), owner);
        check_eq!(edit.expected()?, None);
        check_eq!(edit.next()?, Some((*block).into()));
    }
    check!(cursor.next_edit(context.budget)?.is_none());
    drop(cursor);
    drop(context);
    drop(edits);
    drop(spool);
    check_eq!(memory.live.load(Ordering::Relaxed), 0);
    check!(!memory.invalid_release.load(Ordering::Relaxed));
    Ok(())
}
