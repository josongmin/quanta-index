use super::*;

// Counts calls to the immutable-object port, not device reads or file opens.
struct CountedOccurrencePort<'a> {
    inner: &'a TestObjects,
    reads: Mutex<BTreeMap<ObjectRef, usize>>,
}

impl CountedOccurrencePort<'_> {
    fn reads_for(&self, reference: ObjectRef) -> Result<usize, IndexError> {
        let reads = self.reads.lock().map_err(|_error| {
            IndexError::BackendViolation("occurrence read ledger poisoned".into())
        })?;
        let count = reads.get(&reference).copied().unwrap_or(0);
        drop(reads);
        Ok(count)
    }
}

impl ImmutableObjectPort for CountedOccurrencePort<'_> {
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
        {
            let mut reads = self.reads.lock().map_err(|_error| {
                IndexError::BackendViolation("occurrence read ledger poisoned".into())
            })?;
            let entry = reads.entry(reference).or_default();
            *entry = entry.checked_add(1).ok_or(IndexError::ResourceLimited)?;
            drop(reads);
        }
        self.inner.get(reference, budget)
    }
}

fn large_budget() -> Result<(WorkBudget, Arc<TokenMemory>), IndexError> {
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
    Ok((work, memory))
}

fn run_inventory_for_65_distinct_references(limit: usize) -> Result<(), IndexError> {
    let mut fixture = visitor_fixture()?;
    let root = MapRoot(Some(reference(182)));
    let mut references = Vec::new();
    let mut term_keys = Vec::new();
    for index in 0..65usize {
        let id = 190usize
            .checked_add(index)
            .ok_or(IndexError::ResourceLimited)?;
        let byte = u8::try_from(id).map_err(|_error| IndexError::ResourceLimited)?;
        let occurrence_ref = reference(byte);
        fixture.objects.add(
            occurrence_ref,
            encode(&Occurrence {
                positions: Positions(vec![
                    u32::try_from(index).map_err(|_error| IndexError::ResourceLimited)?,
                ]),
            })?,
            Vec::new(),
        )?;
        let term = format!("term{index:03}");
        for case in [CaseMode::Sensitive, CaseMode::Folded] {
            let key = stored_term_key(&term, case)?;
            fixture.maps.add(root, key.clone(), occurrence_ref)?;
            term_keys.push((key, occurrence_ref));
        }
        references.push(occurrence_ref);
    }
    let next = Contribution {
        record: fixture.source_ref,
        inventory: root.into(),
        indexed_tokens: 65,
    };
    let second_owner = OwnerKey {
        stable_id: StableOwnerId([52; 16]),
        ..fixture.owner
    };
    let mut owners = [owner_key(fixture.owner), owner_key(second_owner)];
    owners.sort_unstable();
    let (mut work, memory) = large_budget()?;
    {
        let scratch = TestScratch::default();
        let custody = TestCustody;
        let spool = TextSpool::new(&scratch, &mut work)?;
        let attempt = test_attempt(fixture.ticket.view.identity.workspace);
        let mut confirmed = ConfirmedOccurrences::new(limit)?;
        if limit != 0 {
            confirmed.prepare(attempt, &work)?;
            // The fixture's immutable leaves are already installed. Model the
            // post-batch identity-verified state consumed by diff_inventory.
            for occurrence_ref in references.iter().take(64) {
                confirmed.remember(*occurrence_ref, attempt, &work)?;
            }
            for group in references.chunks(64) {
                spool.admit_confirmed_occurrences(group, attempt, &mut work)?;
                spool.record_confirmed_occurrences(group, attempt, &work)?;
            }
        }
        let port = CountedOccurrencePort {
            inner: &fixture.objects,
            reads: Mutex::new(BTreeMap::new()),
        };
        let mut edits = PostingEdits::new()?;
        {
            let mut context = AttemptContext {
                attempt,
                objects: &port,
                maps: &fixture.maps,
                custody: &custody,
                scratch: &scratch,
                budget: &mut work,
            };
            for owner in owners {
                diff_inventory(
                    &owner,
                    None,
                    Some(&next),
                    &mut context,
                    &spool,
                    &mut edits,
                    &mut confirmed,
                )?;
            }
            edits.flush(&spool, context.budget)?;
            for occurrence_ref in &references {
                check_eq!(
                    port.reads_for(*occurrence_ref)?,
                    if limit == 0 { 4 } else { 1 }
                );
            }
            for (term, occurrence_ref) in &term_keys {
                let mut cursor = spool.posting_root_cursor(term);
                for owner in owners {
                    let row = cursor.next_edit(context.budget)?.ok_or_else(|| {
                        IndexError::BackendViolation("expected posting edit absent".into())
                    })?;
                    check_eq!(row.edit.key.as_slice(), owner.as_slice());
                    check_eq!(row.edit.expected, None);
                    check_eq!(row.edit.next, Some((*occurrence_ref).into()));
                }
                check!(cursor.next_edit(context.budget)?.is_none());
            }
        }
    }
    check_eq!(memory.live.load(Ordering::Relaxed), 0);
    check!(!memory.invalid_release.load(Ordering::Relaxed));
    Ok(())
}

#[test]
fn confirmed_spill_keeps_65th_reference_at_one_full_get_across_surfaces_and_owners()
-> Result<(), IndexError> {
    run_inventory_for_65_distinct_references(64)?;
    run_inventory_for_65_distinct_references(0)
}

#[test]
fn confirmed_spill_does_not_hide_corrupt_prior_leaf() -> Result<(), IndexError> {
    let mut fixture = visitor_fixture()?;
    fixture.objects.counted_occurrence = Some(fixture.occurrence_ref);
    let attempt = test_attempt(fixture.ticket.view.identity.workspace);
    let (mut work, memory) = large_budget()?;
    {
        let scratch = TestScratch::default();
        let custody = TestCustody;
        let spool = TextSpool::new(&scratch, &mut work)?;
        let mut confirmed = ConfirmedOccurrences::new(64)?;
        confirmed.prepare(attempt, &work)?;
        for byte in 100..164u8 {
            let filler = reference(byte);
            fixture.objects.add(
                filler,
                encode(&Occurrence {
                    positions: Positions(vec![0]),
                })?,
                Vec::new(),
            )?;
            confirmed.remember(filler, attempt, &work)?;
        }
        spool.admit_confirmed_occurrences(&[fixture.occurrence_ref], attempt, &mut work)?;
        spool.record_confirmed_occurrences(&[fixture.occurrence_ref], attempt, &work)?;
        let next = Contribution {
            record: fixture.source_ref,
            inventory: fixture.inventory_root.into(),
            indexed_tokens: 2,
        };
        let mut edits = PostingEdits::new()?;
        {
            let mut context = AttemptContext {
                attempt,
                objects: &fixture.objects,
                maps: &fixture.maps,
                custody: &custody,
                scratch: &scratch,
                budget: &mut work,
            };
            diff_inventory(
                &owner_key(fixture.owner),
                None,
                Some(&next),
                &mut context,
                &spool,
                &mut edits,
                &mut confirmed,
            )?;
            check_eq!(fixture.objects.occurrence_reads.load(Ordering::Relaxed), 1);
            check_eq!(
                spool.confirmed_occurrence_count(
                    fixture.occurrence_ref,
                    attempt,
                    context.budget,
                )?,
                super::super::super::text_spool::ConfirmedOccurrenceCount::Validated(2)
            );
        }
        // The prior contribution is read through the immutable port even
        // when the same reference has an attempt-local positive count.
        fixture.objects.add(
            fixture.occurrence_ref,
            b"{\"positions\":[]}".to_vec(),
            Vec::new(),
        )?;
        let mut context = AttemptContext {
            attempt,
            objects: &fixture.objects,
            maps: &fixture.maps,
            custody: &custody,
            scratch: &scratch,
            budget: &mut work,
        };
        check!(matches!(
            diff_inventory(
                &owner_key(fixture.owner),
                Some(&next),
                None,
                &mut context,
                &spool,
                &mut edits,
                &mut confirmed,
            ),
            Err(IndexError::CorruptObject(_))
        ));
        check_eq!(fixture.objects.occurrence_reads.load(Ordering::Relaxed), 2);
    }
    check_eq!(memory.live.load(Ordering::Relaxed), 0);
    check!(!memory.invalid_release.load(Ordering::Relaxed));
    Ok(())
}

#[test]
fn missing_unprepared_and_disabled_counts_keep_the_original_read_path() -> Result<(), IndexError> {
    let mut fixture = visitor_fixture()?;
    fixture.objects.counted_occurrence = Some(fixture.occurrence_ref);
    let attempt = test_attempt(fixture.ticket.view.identity.workspace);
    let scratch = TestScratch::default();
    let custody = TestCustody;
    let (mut work, memory) = large_budget()?;
    {
        let spool = TextSpool::new(&scratch, &mut work)?;
        let mut prepared_missing = ConfirmedOccurrences::new(64)?;
        prepared_missing.prepare(attempt, &work)?;
        let mut unprepared = ConfirmedOccurrences::new(64)?;
        let mut disabled = ConfirmedOccurrences::new(0)?;
        let mut context = AttemptContext {
            attempt,
            objects: &fixture.objects,
            maps: &fixture.maps,
            custody: &custody,
            scratch: &scratch,
            budget: &mut work,
        };
        for confirmed in [&mut prepared_missing, &mut unprepared, &mut disabled] {
            for _ in 0..2 {
                check_eq!(
                    confirmed.validated_new_count(
                        fixture.term.as_slice(),
                        fixture.occurrence_ref,
                        &spool,
                        &mut context,
                    )?,
                    2
                );
            }
        }
        check_eq!(fixture.objects.occurrence_reads.load(Ordering::Relaxed), 6);
        check_eq!(
            spool.confirmed_occurrence_count(fixture.occurrence_ref, attempt, context.budget)?,
            super::super::super::text_spool::ConfirmedOccurrenceCount::Missing
        );
    }
    check_eq!(memory.live.load(Ordering::Relaxed), 0);
    check!(!memory.invalid_release.load(Ordering::Relaxed));
    Ok(())
}

#[test]
fn spill_count_requires_a_successful_first_decode_and_keeps_fence_and_cancel_guards()
-> Result<(), IndexError> {
    use super::super::super::text_spool::ConfirmedOccurrenceCount;

    let mut fixture = visitor_fixture()?;
    fixture.objects.counted_occurrence = Some(fixture.occurrence_ref);
    fixture.objects.add(
        fixture.occurrence_ref,
        b"{\"positions\":[]}".to_vec(),
        Vec::new(),
    )?;
    let attempt = test_attempt(fixture.ticket.view.identity.workspace);
    let foreign = AttemptRef {
        fence: attempt
            .fence
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?,
        ..attempt
    };
    let scratch = TestScratch::default();
    let custody = TestCustody;
    let (mut work, memory) = large_budget()?;
    {
        let spool = TextSpool::new(&scratch, &mut work)?;
        let mut confirmed = ConfirmedOccurrences::new(64)?;
        confirmed.prepare(attempt, &work)?;
        for byte in 100..164u8 {
            confirmed.remember(reference(byte), attempt, &work)?;
        }
        spool.admit_confirmed_occurrences(&[fixture.occurrence_ref], attempt, &mut work)?;
        spool.record_confirmed_occurrences(&[fixture.occurrence_ref], attempt, &work)?;
        {
            let mut context = AttemptContext {
                attempt,
                objects: &fixture.objects,
                maps: &fixture.maps,
                custody: &custody,
                scratch: &scratch,
                budget: &mut work,
            };
            check!(matches!(
                confirmed.validated_new_count(
                    fixture.term.as_slice(),
                    fixture.occurrence_ref,
                    &spool,
                    &mut context,
                ),
                Err(IndexError::CorruptObject(_))
            ));
            check_eq!(
                spool.confirmed_occurrence_count(
                    fixture.occurrence_ref,
                    attempt,
                    context.budget,
                )?,
                ConfirmedOccurrenceCount::Unvalidated
            );
        }
        fixture.objects.add(
            fixture.occurrence_ref,
            encode(&Occurrence {
                positions: Positions(vec![0, 2]),
            })?,
            Vec::new(),
        )?;
        let mut context = AttemptContext {
            attempt,
            objects: &fixture.objects,
            maps: &fixture.maps,
            custody: &custody,
            scratch: &scratch,
            budget: &mut work,
        };
        check_eq!(
            confirmed.validated_new_count(
                fixture.term.as_slice(),
                fixture.occurrence_ref,
                &spool,
                &mut context,
            )?,
            2
        );
        check_eq!(fixture.objects.occurrence_reads.load(Ordering::Relaxed), 2);
        check_eq!(
            spool.confirmed_occurrence_count(fixture.occurrence_ref, attempt, context.budget)?,
            ConfirmedOccurrenceCount::Validated(2)
        );
        context.attempt = foreign;
        check!(matches!(
            confirmed.validated_new_count(
                fixture.term.as_slice(),
                fixture.occurrence_ref,
                &spool,
                &mut context,
            ),
            Err(IndexError::FenceLost)
        ));
        context.attempt = attempt;
        context.budget.request().cancel_handle().cancel();
        check!(matches!(
            confirmed.validated_new_count(
                fixture.term.as_slice(),
                fixture.occurrence_ref,
                &spool,
                &mut context,
            ),
            Err(IndexError::Cancelled)
        ));
        check_eq!(fixture.objects.occurrence_reads.load(Ordering::Relaxed), 2);
    }
    check_eq!(memory.live.load(Ordering::Relaxed), 0);
    check!(!memory.invalid_release.load(Ordering::Relaxed));
    Ok(())
}
