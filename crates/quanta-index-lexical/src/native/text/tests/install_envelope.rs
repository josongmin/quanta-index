use super::*;

#[derive(serde::Serialize)]
struct OnePosition {
    positions: [u32; 1],
}

struct CreditAdmissionScope {
    memory: Arc<TokenMemory>,
    accepted: Arc<AtomicUsize>,
    ceiling: Arc<AtomicU64>,
}

struct CreditAdmissionPool {
    grant: Mutex<Option<quanta_index_core::index::MemoryLease>>,
    usage: Mutex<(u64, u32)>,
    limits: quanta_index_core::index::MemoryCreditLimits,
}

struct CreditAdmissionChild(Arc<CreditAdmissionPool>);

impl quanta_index_core::index::MemoryLeaseOwner for CreditAdmissionPool {
    fn release(&self, bytes: u64) {
        let mut usage = self.usage.lock().expect("test credit usage lock poisoned");
        usage.0 -= bytes;
        usage.1 -= 1;
    }
}

impl quanta_index_core::index::MemoryScopePort for CreditAdmissionChild {
    fn reserve(
        &self,
        bytes: u64,
        lifetime: quanta_index_core::index::MemoryLifetime,
    ) -> Result<quanta_index_core::index::MemoryLease, IndexError> {
        if lifetime != quanta_index_core::index::MemoryLifetime::Temporary {
            return Err(IndexError::ResourceLimited);
        }
        let mut usage = self
            .0
            .usage
            .lock()
            .expect("test credit usage lock poisoned");
        let total = usage
            .0
            .checked_add(bytes)
            .filter(|total| *total <= self.0.limits.temporary_data_bytes)
            .ok_or(IndexError::ResourceLimited)?;
        let count = usage
            .1
            .checked_add(1)
            .filter(|count| *count <= self.0.limits.max_live_temporary_leases)
            .ok_or(IndexError::ResourceLimited)?;
        *usage = (total, count);
        drop(usage);
        Ok(quanta_index_core::index::MemoryLease::new(
            bytes,
            self.0.clone(),
        ))
    }
}

impl quanta_index_core::index::MemoryCreditClosePort for CreditAdmissionChild {
    fn close(&self) {
        let _released = self
            .0
            .grant
            .lock()
            .expect("test credit lock poisoned")
            .take();
    }
}

impl quanta_index_core::index::MemoryScopePort for CreditAdmissionScope {
    fn reserve(
        &self,
        bytes: u64,
        _lifetime: quanta_index_core::index::MemoryLifetime,
    ) -> Result<quanta_index_core::index::MemoryLease, IndexError> {
        let _total = self
            .memory
            .live
            .load(Ordering::Relaxed)
            .checked_add(bytes)
            .filter(|total| *total <= self.ceiling.load(Ordering::Relaxed))
            .ok_or(IndexError::ResourceLimited)?;
        token_memory_grant(&self.memory, bytes)
    }

    fn try_reserve_credit(
        &self,
        limits: quanta_index_core::index::MemoryCreditLimits,
    ) -> Result<Option<quanta_index_core::index::MemoryCredit>, IndexError> {
        let bytes = limits
            .temporary_data_bytes
            .checked_add(limits.retained_data_bytes)
            .ok_or(IndexError::ResourceLimited)?;
        if !self
            .memory
            .live
            .load(Ordering::Relaxed)
            .checked_add(bytes)
            .is_some_and(|total| total <= self.ceiling.load(Ordering::Relaxed))
        {
            return Ok(None);
        }
        let grant = match token_memory_grant(&self.memory, bytes) {
            Ok(grant) => grant,
            Err(IndexError::ResourceLimited) => return Ok(None),
            Err(error) => return Err(error),
        };
        let _previous = self.accepted.fetch_add(1, Ordering::Relaxed);
        let child = Arc::new(CreditAdmissionChild(Arc::new(CreditAdmissionPool {
            grant: Mutex::new(Some(grant)),
            usage: Mutex::new((0, 0)),
            limits,
        })));
        Ok(Some(quanta_index_core::index::MemoryCredit::new(
            quanta_index_core::index::MemoryScope::new(child.clone()),
            child,
        )))
    }
}

fn stage(
    writes: &mut OccurrenceWrites<'_>,
    edits: &mut InventoryEdits,
    spool: &TextSpool,
    slot: u16,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    let byte = u8::try_from(slot).map_err(|_error| IndexError::ResourceLimited)?;
    let key = format!("\0term-{slot:03}");
    let payload = super::super::super::text_memory::encode(
        &OnePosition {
            positions: [u32::from(slot)],
        },
        budget,
    )?;
    writes.push(
        OccurrenceEdit {
            ordinal: u32::from(slot.checked_rem(64).ok_or(IndexError::ResourceLimited)?),
            key: PreparedKey::copy(key.as_bytes(), budget)?,
            prior: None,
            identified: reference(byte),
        },
        payload,
        spool,
        edits,
        budget,
    )
}

#[test]
fn tiny_cold_packet_uses_its_physical_install_shape() -> Result<(), IndexError> {
    let fixture = visitor_fixture()?;
    let scratch = TestScratch::default();
    let custody = TestCustody;
    let mut work = budget()?;
    let spool = TextSpool::new(&scratch, &mut work)?;
    let writes = OccurrenceWrites::new();
    let shape = super::super::cold_tokens::ColdBlockShape {
        object_count: 1,
        total_payload_bytes: 128,
        maximum_payload_bytes: 128,
        maximum_key_bytes: 8,
        term_count: 2,
        maximum_key_position_peak: 64,
        maximum_group_positions: 1,
    };
    let packet = super::super::occurrence_pack::cold_object_install_shape(shape, &writes)?
        .ok_or(IndexError::ResourceLimited)?;
    check_eq!(packet.maximum_inputs, 1);
    check_eq!(packet.maximum_total_payload_bytes, 128);
    check_eq!(packet.maximum_payload_bytes, 128);
    let objects = TestObjects {
        bounded_install_memory: true,
        ..TestObjects::default()
    };
    let context = AttemptContext {
        attempt: test_attempt(fixture.ticket.view.identity.workspace),
        objects: &objects,
        maps: &fixture.maps,
        custody: &custody,
        scratch: &scratch,
        budget: &mut work,
    };
    let selected =
        super::super::occurrence_pack::cold_output_credit_bound(shape, &context, &spool, &writes)?
            .ok_or(IndexError::ResourceLimited)?;
    let fixed = context
        .put_batch_memory_bound(quanta_index_core::index::ports::ObjectInstallMemoryShape {
            maximum_inputs: quanta_index_core::index::ports::MAX_OBJECT_INSTALL_BATCH,
            maximum_total_payload_bytes: usize::try_from(MAX_OCCURRENCE_STAGED_BYTES)
                .map_err(|_error| IndexError::ResourceLimited)?,
            maximum_payload_bytes: 128,
            maximum_total_children: 0,
        })?
        .ok_or(IndexError::ResourceLimited)?;
    let actual = context
        .put_batch_memory_bound(packet)?
        .ok_or(IndexError::ResourceLimited)?;
    let old = super::super::occurrence_pack::owned_cold_memory_bound(shape)?
        .checked_add(spool.inventory_confirmation_memory_bound(
            quanta_index_core::index::ports::MAX_OBJECT_INSTALL_BATCH,
        )?)?
        .checked_add(quanta_index_scratch_sqlite::map_edit_sql_memory_bound(
            "INSERT INTO inventory(root,k,p,n) VALUES ",
            quanta_index_scratch_sqlite::MAX_WRITE_BATCH_ROWS,
        )?)?
        .checked_add(fixed)?;
    let selected_bytes = selected
        .temporary_data_bytes
        .checked_add(selected.retained_data_bytes)
        .ok_or(IndexError::ResourceLimited)?;
    let old_object_bytes = fixed
        .temporary_data_bytes
        .checked_add(fixed.retained_data_bytes)
        .ok_or(IndexError::ResourceLimited)?;
    let new_object_bytes = actual
        .temporary_data_bytes
        .checked_add(actual.retained_data_bytes)
        .ok_or(IndexError::ResourceLimited)?;
    check!(old_object_bytes > new_object_bytes);
    let old_bytes = old
        .temporary_data_bytes
        .checked_add(old.retained_data_bytes)
        .ok_or(IndexError::ResourceLimited)?;
    let saved_bytes = old_bytes
        .checked_sub(selected_bytes)
        .filter(|saved| *saved > 1)
        .ok_or(IndexError::ResourceLimited)?;
    // A cap between the two complete credit bounds witnesses the admission
    // difference; the independent one-object/128-byte packet gold is above.
    let original_cap = selected_bytes
        .checked_add(saved_bytes / 2)
        .ok_or(IndexError::ResourceLimited)?;
    check!(selected_bytes < original_cap && original_cap < old_bytes);
    drop(context);
    let memory = Arc::new(TokenMemory {
        ceiling: original_cap,
        ..TokenMemory::default()
    });
    work.bind_memory_scope(Some(quanta_index_core::index::MemoryScope::new(Arc::new(
        CreditAdmissionScope {
            memory: Arc::clone(&memory),
            accepted: Arc::new(AtomicUsize::new(0)),
            ceiling: Arc::new(AtomicU64::new(u64::MAX)),
        },
    ))));
    check!(work.try_scoped_memory_credit(old)?.is_none());
    {
        let admitted = work
            .try_scoped_memory_credit(selected)?
            .ok_or(IndexError::ResourceLimited)?;
        check_eq!(memory.live.load(Ordering::Relaxed), selected_bytes);
        drop(admitted);
    }
    check_eq!(memory.live.load(Ordering::Relaxed), 0);
    let admitted_again = work
        .try_scoped_memory_credit(selected)?
        .ok_or(IndexError::ResourceLimited)?;
    drop(admitted_again);
    check_eq!(memory.live.load(Ordering::Relaxed), 0);
    check_eq!(fixture.objects.install_batches.load(Ordering::Relaxed), 0);
    Ok(())
}

#[test]
fn cold_packet_bound_includes_pending_payload_and_flush_peak() -> Result<(), IndexError> {
    let scratch = TestScratch::default();
    let mut work = WorkBudget::new(
        WorkLimits {
            reads: 4096,
            writes: 4096,
            bytes: 16_000_000,
            mutations: 4096,
        },
        Instant::now()
            .checked_add(Duration::from_secs(30))
            .ok_or(IndexError::ResourceLimited)?,
    );
    let spool = TextSpool::new(&scratch, &mut work)?;
    let mut writes = OccurrenceWrites::new();
    let mut edits = InventoryEdits::new(0)?;
    let payload = super::super::super::text_memory::encode(&"x".repeat(900_000), &work)?;
    let pending_bytes = payload.as_slice().len();
    writes.push(
        OccurrenceEdit {
            ordinal: 0,
            key: PreparedKey::copy(b"\0pending", &work)?,
            prior: None,
            identified: reference(55),
        },
        payload,
        &spool,
        &mut edits,
        &mut work,
    )?;
    let shape = super::super::cold_tokens::ColdBlockShape {
        object_count: 300,
        total_payload_bytes: 1_500_000,
        maximum_payload_bytes: 8_192,
        maximum_key_bytes: 8,
        term_count: 300,
        maximum_key_position_peak: 64,
        maximum_group_positions: 1,
    };
    let packet = super::super::occurrence_pack::cold_object_install_shape(shape, &writes)?
        .ok_or(IndexError::ResourceLimited)?;
    check_eq!(
        packet.maximum_inputs,
        quanta_index_core::index::ports::MAX_OBJECT_INSTALL_BATCH
    );
    check_eq!(
        packet.maximum_total_payload_bytes,
        usize::try_from(MAX_OCCURRENCE_STAGED_BYTES)
            .map_err(|_error| IndexError::ResourceLimited)?
    );
    check_eq!(packet.maximum_payload_bytes, pending_bytes);
    check_eq!(packet.maximum_total_children, 0);
    Ok(())
}

#[test]
fn cold_owner_borrows_sorted_arena_without_replay_credit() -> Result<(), IndexError> {
    let body = "token ".repeat(257);
    let fixture = visitor_fixture()?;
    let scratch = TestScratch::default();
    let custody = TestCustody;
    let mut work = WorkBudget::new(
        WorkLimits {
            reads: 100_000,
            writes: 100_000,
            bytes: 100_000_000,
            mutations: 257,
        },
        Instant::now()
            .checked_add(Duration::from_secs(30))
            .ok_or(IndexError::ResourceLimited)?,
    );
    let spool = TextSpool::new(&scratch, &mut work)?;
    let memory = Arc::new(TokenMemory {
        ceiling: 16 * 1024 * 1024,
        ..TokenMemory::default()
    });
    let credits = Arc::new(AtomicUsize::new(0));
    let ceiling = Arc::new(AtomicU64::new(u64::MAX));
    work.bind_memory_scope(Some(quanta_index_core::index::MemoryScope::new(Arc::new(
        CreditAdmissionScope {
            memory: Arc::clone(&memory),
            accepted: Arc::clone(&credits),
            ceiling: Arc::clone(&ceiling),
        },
    ))));
    let mut cursor = prepare_indexed_token_cursor(&body, &mut work)?;
    let census =
        super::super::cold_tokens::CanonicalTokenCensus::count(&mut cursor, body.len(), &mut work)?
            .ok_or(IndexError::ResourceLimited)?;
    let mut arena = super::super::cold_tokens::ColdTokens::try_new(&census, &work)?
        .ok_or(IndexError::ResourceLimited)?;
    check_eq!(arena.fill_from_cursor(&mut cursor, &mut work)?, 257);
    arena.sort_and_validate(&work)?;
    check!(spool.token_term_page(None, &mut work)?.is_empty());
    let live = memory.live.load(Ordering::Relaxed);
    ceiling.store(live, Ordering::Relaxed);
    // No additional Original byte can be granted while the validated arena
    // is live. SQL replay must borrow it without a packet or string grant.
    let mut cold = Some(arena);
    let objects = TestObjects::default(); // No ObjectInstallMemoryShape bound.
    let mut writes = OccurrenceWrites::new();
    writes.pack_owners = true;
    let mut edits = InventoryEdits::new(0)?;
    let mut confirmed = ConfirmedOccurrences::new(0)?;
    check!(writes.inputs.is_empty() && edits.rows.as_slice().is_empty());
    check_eq!(objects.install_batches.load(Ordering::Relaxed), 0);
    let mut context = AttemptContext {
        attempt: test_attempt(fixture.ticket.view.identity.workspace),
        objects: &objects,
        maps: &fixture.maps,
        custody: &custody,
        scratch: &scratch,
        budget: &mut work,
    };
    let prepared = super::super::occurrence_pack::prepare_owner_blocks(
        0,
        &mut context,
        &spool,
        &mut cold,
        &mut confirmed,
        &mut edits,
        &mut writes,
        None,
    );
    let (term_count, alias) = prepared.map_err(|error| {
        IndexError::BackendViolation(format!(
            "cold replay preparation {error:?}, {:?}",
            context.budget
        ))
    })?;
    check_eq!(term_count, 2);
    check!(alias.is_none() && cold.is_none());
    check_eq!(credits.load(Ordering::Relaxed), 0);
    check!(context.budget.first_denial().is_none());
    check_eq!(context.budget.usage().mutations, 257);
    check_eq!(objects.install_batches.load(Ordering::Relaxed), 0);
    let replay_payloads: Vec<_> = writes
        .inputs
        .iter()
        .map(|input| input.payload.clone())
        .collect();
    drop(context);
    ceiling.store(u64::MAX, Ordering::Relaxed);
    let mut replay_groups = BTreeMap::new();
    let readback = spool.visit_token_groups(8192, &mut work, |key, positions, _budget| {
        let _prior = replay_groups.insert(key.as_slice().to_vec(), positions.as_slice().to_vec());
        Ok(())
    });
    readback.map_err(|error| {
        IndexError::BackendViolation(format!("cold replay SQL readback {error:?}, {work:?}"))
    })?;
    check_eq!(replay_groups.len(), 2);
    let expected_positions = (0_u32..257).collect::<Vec<_>>();
    for case in [CaseMode::Sensitive, CaseMode::Folded] {
        check_eq!(
            replay_groups.get(&stored_term_key("token", case)?),
            Some(&expected_positions)
        );
    }

    let baseline_scratch = TestScratch::default();
    let mut baseline_work = WorkBudget::new(
        WorkLimits {
            reads: 100_000,
            writes: 100_000,
            bytes: 100_000_000,
            mutations: 257,
        },
        Instant::now()
            .checked_add(Duration::from_secs(30))
            .ok_or(IndexError::ResourceLimited)?,
    );
    let baseline_spool = TextSpool::new(&baseline_scratch, &mut baseline_work)?;
    baseline_spool.reset_tokens(&mut baseline_work)?;
    check_eq!(
        visit_indexed_token_batches(&body, &mut baseline_work, |batch, budget| {
            baseline_spool.token_batch(batch, budget)
        })?,
        257
    );
    let mut baseline_groups = BTreeMap::new();
    baseline_spool.visit_token_groups(8192, &mut baseline_work, |key, positions, _budget| {
        let _prior = baseline_groups.insert(key.as_slice().to_vec(), positions.as_slice().to_vec());
        Ok(())
    })?;
    check_eq!(replay_groups, baseline_groups);
    let mut baseline_writes = OccurrenceWrites::new();
    baseline_writes.pack_owners = true;
    let mut baseline_edits = InventoryEdits::new(0)?;
    let mut baseline_confirmed = ConfirmedOccurrences::new(0)?;
    let mut no_cold = None;
    let mut baseline_context = AttemptContext {
        attempt: test_attempt(fixture.ticket.view.identity.workspace),
        objects: &objects,
        maps: &fixture.maps,
        custody: &custody,
        scratch: &baseline_scratch,
        budget: &mut baseline_work,
    };
    check_eq!(
        super::super::occurrence_pack::prepare_owner_blocks(
            0,
            &mut baseline_context,
            &baseline_spool,
            &mut no_cold,
            &mut baseline_confirmed,
            &mut baseline_edits,
            &mut baseline_writes,
            None,
        )?
        .0,
        term_count
    );
    check_eq!(
        replay_payloads,
        baseline_writes
            .inputs
            .iter()
            .map(|input| input.payload.clone())
            .collect::<Vec<_>>()
    );
    drop((
        baseline_context,
        baseline_writes,
        baseline_edits,
        baseline_spool,
    ));
    drop((writes, edits, confirmed, spool, cursor));
    check_eq!(memory.live.load(Ordering::Relaxed), 0);
    Ok(())
}

#[test]
fn one_256_input_install_promotes_all_ordinals_and_keeps_hot_memo_at_64() -> Result<(), IndexError>
{
    let mut fixture = visitor_fixture()?;
    fixture.objects.install_outputs = Some(
        (0_u16..256)
            .map(|slot| {
                u8::try_from(slot)
                    .map(reference)
                    .map_err(|_error| IndexError::ResourceLimited)
            })
            .collect::<Result<Vec<_>, _>>()?,
    );
    let scratch = TestScratch::default();
    let custody = TestCustody;
    let mut work = WorkBudget::new(
        WorkLimits {
            reads: 4096,
            writes: 4096,
            bytes: 16_000_000,
            mutations: 4096,
        },
        Instant::now()
            .checked_add(Duration::from_secs(30))
            .ok_or(IndexError::ResourceLimited)?,
    );
    let spool = TextSpool::new(&scratch, &mut work)?;
    let mut writes = OccurrenceWrites::new();
    let mut edits = InventoryEdits::new(0)?;
    for slot in 0_u16..256 {
        stage(&mut writes, &mut edits, &spool, slot, &mut work)?;
        if slot == 0 {
            let cap = usize::try_from(MAX_OCCURRENCE_STAGED_BYTES)
                .map_err(|_error| IndexError::ResourceLimited)?;
            check!(writes.needs_flush(reference(1), cap)?);
            check!(!writes.needs_flush(reference(0), cap)?);
        }
    }
    check_eq!(writes.pending.len(), 256);
    check!(!writes.needs_flush(reference(255), 1)?);
    let mut unknown = [255; 32];
    *unknown.get_mut(0).ok_or(IndexError::ResourceLimited)? = 0;
    check!(writes.needs_flush(ObjectRef(Digest(unknown)), 1)?);
    check_eq!(fixture.objects.install_batches.load(Ordering::Relaxed), 0);
    let attempt = test_attempt(fixture.ticket.view.identity.workspace);
    let mut confirmed = ConfirmedOccurrences::new(64)?;
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
        256
    );
    check_eq!(confirmed.references.len(), 64);
    check!(confirmed.contains_or_spill(reference(255), attempt, &spool, &mut work)?);
    check!(writes.pending.is_empty());
    let mut cursor = spool.inventory_cursor(64)?;
    let mut seen = [false; 256];
    for _ in 0..256 {
        let row = cursor.next_edit(&mut work)?.ok_or_else(|| {
            IndexError::BackendViolation("missing installed occurrence edit".into())
        })?;
        let text = std::str::from_utf8(&row.key())
            .map_err(|_error| IndexError::CorruptObject("invalid fixed key".into()))?;
        let slot: u16 = text
            .strip_prefix("\0term-")
            .ok_or_else(|| IndexError::CorruptObject("invalid fixed term".into()))?
            .parse()
            .map_err(|_error| IndexError::CorruptObject("invalid fixed slot".into()))?;
        let index = usize::from(slot);
        let visited = seen.get_mut(index).ok_or(IndexError::ResourceLimited)?;
        check!(!*visited);
        *visited = true;
        check_eq!(
            row.root(),
            u32::from(slot.checked_rem(64).ok_or(IndexError::ResourceLimited)?)
        );
        check_eq!(row.expected()?, None);
        check_eq!(
            row.next()?,
            Some(
                reference(u8::try_from(slot).map_err(|_error| IndexError::ResourceLimited)?).into()
            )
        );
    }
    check!(seen.iter().all(|visited| *visited));
    check!(cursor.next_edit(&mut work)?.is_none());
    Ok(())
}

#[test]
fn wrong_final_output_keeps_all_256_rows_planned() -> Result<(), IndexError> {
    let mut fixture = visitor_fixture()?;
    let mut outputs: Vec<_> = (0_u16..256)
        .map(|slot| {
            u8::try_from(slot)
                .map(reference)
                .map_err(|_error| IndexError::ResourceLimited)
        })
        .collect::<Result<Vec<_>, _>>()?;
    *outputs.last_mut().ok_or(IndexError::ResourceLimited)? = ObjectRef(Digest([42; 32]));
    fixture.objects.install_outputs = Some(outputs);
    let scratch = TestScratch::default();
    let custody = TestCustody;
    let mut work = WorkBudget::new(
        WorkLimits {
            reads: 4096,
            writes: 4096,
            bytes: 16_000_000,
            mutations: 4096,
        },
        Instant::now()
            .checked_add(Duration::from_secs(30))
            .ok_or(IndexError::ResourceLimited)?,
    );
    let spool = TextSpool::new(&scratch, &mut work)?;
    let mut writes = OccurrenceWrites::new();
    let mut edits = InventoryEdits::new(0)?;
    for slot in 0_u16..256 {
        stage(&mut writes, &mut edits, &spool, slot, &mut work)?;
    }
    let attempt = test_attempt(fixture.ticket.view.identity.workspace);
    let mut confirmed = ConfirmedOccurrences::new(64)?;
    check!(matches!(
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
        ),
        Err(IndexError::BackendViolation(message))
            if message == "native text occurrence identity changed during installation"
    ));
    check_eq!(
        fixture.objects.install_batch_inputs.load(Ordering::Relaxed),
        256
    );
    check_eq!(writes.pending.len(), 256);
    check!(confirmed.references.is_empty());
    check!(!confirmed.contains_or_spill(reference(0), attempt, &spool, &mut work)?);
    check!(!confirmed.contains_or_spill(reference(255), attempt, &spool, &mut work)?);
    check!(spool.inventory_cursor(64)?.next_edit(&mut work).is_err());
    Ok(())
}
