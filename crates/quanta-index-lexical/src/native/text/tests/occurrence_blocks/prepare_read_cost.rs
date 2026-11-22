//! Physical read regressions for the existing Caddy and etcd prepare paths.

use super::*;
use quanta_index_core::DiagnosticCounterV1;
use quanta_index_core::index::{MemoryLease, RegisteredObjectBatch, WorkBudgetDenial};

fn prepare_budget() -> WorkBudget {
    WorkBudget::with_request(
        WorkLimits {
            reads: 1_000_000,
            writes: 1_000_000,
            bytes: 512 * 1024 * 1024,
            mutations: 1_000_000,
        },
        RequestBudgetV1::for_duration(Duration::from_secs(60)),
    )
}

#[derive(Debug, Default)]
struct PostingTrace(Mutex<Vec<DiagnosticEmitV1>>);

impl DiagnosticEventSinkV1 for PostingTrace {
    fn try_emit(&self, event: DiagnosticEmitV1) -> bool {
        let Ok(mut events) = self.0.try_lock() else {
            return false;
        };
        events.push(event);
        true
    }
}

#[derive(Debug, Default)]
struct Tick(AtomicU64);

impl DiagnosticClockV1 for Tick {
    fn now_ns(&self) -> u64 {
        self.0.fetch_add(1, Ordering::Relaxed)
    }

    fn thread_cpu_now_ns(&self) -> Option<u64> {
        None
    }
}

impl PostingTrace {
    fn check_nested_flush_parent(&self) -> Result<(), IndexError> {
        let events = self
            .0
            .lock()
            .map_err(|_error| IndexError::Storage("posting trace lock".into()))?;
        let mut active_posting_write = None;
        let mut nested_flush = false;
        for event in events.iter() {
            match event.kind {
                DiagnosticEventKindV1::SpanBegin {
                    stage: DiagnosticStageV1::NativeTextPostings,
                    lane: DiagnosticLaneV1(38),
                    span_id,
                    ..
                } => {
                    check!(active_posting_write.replace(span_id).is_none());
                }
                DiagnosticEventKindV1::SpanBegin {
                    stage: DiagnosticStageV1::NativeTextPostings,
                    lane: DiagnosticLaneV1(39),
                    parent_span_id,
                    ..
                } => {
                    if let Some(parent) = active_posting_write {
                        check!(
                            parent_span_id == Some(parent),
                            "nested posting flush lost its active Work parent"
                        );
                        nested_flush = true;
                    }
                }
                DiagnosticEventKindV1::SpanEnd { span_id, .. }
                    if active_posting_write == Some(span_id) =>
                {
                    active_posting_write = None;
                }
                _ => {}
            }
        }
        check!(active_posting_write.is_none());
        check!(nested_flush, "fixture did not flush inside posting writes");
        Ok(())
    }

    fn first_root_work(&self) -> Result<u64, IndexError> {
        self.stage_work(
            DiagnosticStageV1::NativeTextPostings,
            DiagnosticLaneV1(37),
            DiagnosticCounterV1::WorkBytes,
        )
    }

    fn stage_work(
        &self,
        phase: DiagnosticStageV1,
        step: DiagnosticLaneV1,
        counter: DiagnosticCounterV1,
    ) -> Result<u64, IndexError> {
        let events = self
            .0
            .lock()
            .map_err(|_error| IndexError::Storage("posting trace lock".into()))?;
        let id = events
            .iter()
            .find_map(|event| match event.kind {
                DiagnosticEventKindV1::SpanBegin {
                    stage,
                    lane,
                    span_id,
                    ..
                } if stage == phase && lane == step => Some(span_id),
                _ => None,
            })
            .ok_or_else(|| IndexError::BackendViolation("expected Work stage absent".into()))?;
        events
            .iter()
            .find_map(|event| match event.kind {
                DiagnosticEventKindV1::Counter {
                    span_id,
                    key,
                    value,
                } if span_id == id && key == counter => Some(value),
                _ => None,
            })
            .ok_or_else(|| IndexError::BackendViolation("expected Work counter absent".into()))
    }
}

fn published_terms(
    objects: &CanonicalObjects,
    maps: &CanonicalMaps,
    root: MapRoot,
    expected_terms: usize,
    budget: &mut WorkBudget,
) -> Result<Rows, IndexError> {
    let pages = maps.rows(root)?;
    // Page count depends on encoded key/value bytes and distinct children,
    // not only row count. Exact terms and interval/registration checks below
    // remain the semantic oracle; fixed wire-boundary tests cover splitting.
    check!(!pages.is_empty());
    check!(pages.len() <= expected_terms);
    let mut terms = Rows::new();
    let mut prior_fence: Option<Vec<u8>> = None;
    for (fence, value) in pages {
        if let Some(prior) = &prior_fence {
            check!(prior < &fence);
        }
        let reference = value.object_ref()?;
        let stored = objects.get(reference, budget)?;
        check!(stored.payload.len() <= 16 * 1024);
        check!(stored.registration.children.len() <= 255);
        let page = term_dictionary::CheckedPage::parse(stored, reference, budget)?;
        page.validate_fence(&fence)?;
        if let Some(prior) = &prior_fence {
            check!(page.first_key()? > prior.as_slice());
        }
        for slot in 0..page.len() {
            let (key, posting) = page.entry(slot)?;
            check!(terms.insert(key.to_vec(), posting).is_none());
        }
        prior_fence = Some(fence);
    }
    check_eq!(terms.len(), expected_terms);
    Ok(terms)
}

fn posting_prepare(tail_width: usize) -> Result<(i32, u64, u64), IndexError> {
    let workspace = visitor_fixture()?.ticket.view.identity.workspace;
    let objects = CanonicalObjects::default();
    let maps = CanonicalMaps::default();
    let scratch = TestScratch::default();
    let mut budget = prepare_budget();
    let spool = TextSpool::new(&scratch, &mut budget)?;
    let owner = [7_u8; 32];
    let occurrence = reference(40);
    let tail = format!("\x01{}", "z".repeat(tail_width)).into_bytes();
    let mut rows = BTreeMap::new();
    for surface in ['\0', '\x01'] {
        for index in 0_u8..33 {
            let _prior = rows.insert(format!("{surface}a{index:02}").into_bytes(), occurrence);
        }
    }
    let _prior = rows.insert(tail, occurrence);
    let edits = rows
        .iter()
        .map(|(term, next)| {
            (
                term.clone(),
                MapEdit::objects(owner.to_vec(), None, Some(*next)),
            )
        })
        .collect::<Vec<_>>();
    for chunk in edits.chunks(quanta_index_scratch_sqlite::MAX_WRITE_BATCH_ROWS) {
        spool.posting_batch(chunk, &mut budget)?;
    }
    // Seal the physical packet before measuring apply_postings.
    check_eq!(spool.posting_term_page(None, &mut budget)?.len(), 64);
    let (before, wire_bytes) = spool.posting_packet_read_probe(&budget)?;
    let trace = Arc::new(PostingTrace::default());
    let diagnostic = DiagnosticContextV1::new(
        DiagnosticCaptureIdV1([31; 16]),
        Arc::new(Tick::default()),
        trace.clone(),
    );
    let parent = diagnostic
        .enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(0))
        .map_err(|error| IndexError::BackendViolation(error.to_string()))?;
    let mut root = Root::default();
    let mut context = AttemptContext {
        attempt: test_attempt(workspace),
        objects: &objects,
        maps: &maps,
        custody: &TestCustody,
        scratch: &scratch,
        budget: &mut budget,
    };
    apply_postings(
        &mut root,
        &mut context,
        &spool,
        Some(&diagnostic.child_context(&parent)),
    )?;
    let (after, observed_bytes) = spool.posting_packet_read_probe(context.budget)?;
    check_eq!(observed_bytes, wire_bytes);
    parent
        .finish(DiagnosticTerminalV1::Ok)
        .map_err(|error| IndexError::BackendViolation(error.to_string()))?;
    let terms = published_terms(&objects, &maps, root.terms, 67, context.budget)?;
    let value = MapValue::inline(&owner, Some(occurrence))?;
    for key in rows.keys() {
        check_eq!(terms.get(key), Some(&value));
    }
    // The next epoch uses a fresh spool over the published base. One old
    // posting is deleted; another moves from the old owner to a new owner.
    // Preserve the original CAS, term order and independent EOF checks.
    let (moved, deleted) = (b"\0a00".as_slice(), b"\x01a00".as_slice());
    let new_owner = [8; 32];
    let new_occurrence = reference(41);
    let delta = TextSpool::new(&scratch, context.budget)?;
    delta.posting_batch(
        &[
            (
                deleted.to_vec(),
                MapEdit::objects(owner.to_vec(), Some(occurrence), None),
            ),
            (
                moved.to_vec(),
                MapEdit::objects(new_owner.to_vec(), None, Some(new_occurrence)),
            ),
            (
                moved.to_vec(),
                MapEdit::objects(owner.to_vec(), Some(occurrence), None),
            ),
        ],
        context.budget,
    )?;
    let old_root = root;
    apply_postings(&mut root, &mut context, &delta, None)?;
    let mut expected = terms.clone();
    let _deleted = expected.remove(deleted);
    let _moved = expected.insert(
        moved.to_vec(),
        MapValue::inline(&new_owner, Some(new_occurrence))?,
    );
    check_eq!(
        published_terms(&objects, &maps, root.terms, expected.len(), context.budget)?,
        expected
    );
    check_eq!(
        published_terms(&objects, &maps, old_root.terms, terms.len(), context.budget)?,
        terms
    );
    trace.check_nested_flush_parent()?;
    Ok((
        after
            .checked_sub(before)
            .ok_or(IndexError::ResourceLimited)?,
        wire_bytes,
        trace.first_root_work()?,
    ))
}

#[test]
fn posting_prepare_reuses_packet_across_case_seeks_within_each_stream() -> Result<(), IndexError> {
    let (reads, wire, work) = posting_prepare(8)?;
    let (wide_reads, wide_wire, wide_work) = posting_prepare(768)?;
    let copy_work = wide_work
        .checked_sub(work)
        .ok_or(IndexError::ResourceLimited)?;
    let width_gain = wide_wire
        .checked_sub(wire)
        .ok_or(IndexError::ResourceLimited)?;
    check_eq!(width_gain, 760);
    // The wider key is beyond both first-page lookaheads. The first page has
    // the same rows, seeks, order witnesses and digests in both runs, so only
    // copying the BLOB can increase its Work. Allow an independent copy for
    // the original stream, and one for root preparation.
    let allowed_copy_work = width_gain
        .checked_mul(2)
        .ok_or(IndexError::ResourceLimited)?;
    // Two posting pages, each with two independent consumers; the final
    // folded range can also probe physical EOF once per consumer.
    check!(
        (2..=6).contains(&reads)
            && wide_reads == reads
            && (width_gain..=allowed_copy_work).contains(&copy_work),
        "posting packet SELECTs={reads}/{wide_reads}; first-page copy Work delta={copy_work}, payload delta={width_gain}"
    );
    Ok(())
}

#[test]
fn posting_shared_block_binds_case_slots_and_preserves_staged_object_posting()
-> Result<(), IndexError> {
    let workspace = visitor_fixture()?.ticket.view.identity.workspace;
    let objects = CanonicalObjects::default();
    let maps = CanonicalMaps::default();
    let scratch = TestScratch::default();
    let mut budget = prepare_budget();
    let spool = TextSpool::new(&scratch, &mut budget)?;
    let occurrence = reference(40);
    // The existing persisted Object posting remains valid under its own key.
    // New short terms use one shared block with four distinct full-key slots.
    let confirmed_rows = [([1; 32], occurrence), ([2; 32], occurrence)];
    let missing_rows = [([3; 32], occurrence), ([4; 32], occurrence)];
    let mut prior_map = entry_map::SmallMap::default();
    for (owner, reference) in confirmed_rows {
        check!(prior_map.push(&owner, reference)?);
    }
    let prior_posting = Posting {
        root: EntryMap::inline(&prior_map),
        df: 2,
    };
    let children = posting_children(&prior_posting)?;
    let confirmed = objects.seed(encode(&prior_posting)?, children.as_slice().to_vec())?;
    spool.top_batch(
        &[(
            0,
            MapEdit::objects(b"\x01staged".to_vec(), None, Some(confirmed)),
        )],
        &mut budget,
    )?;
    let mut edits = Vec::new();
    for (term, rows) in [
        (b"\0a", confirmed_rows),
        (b"\0b", missing_rows),
        (b"\x01a", confirmed_rows),
        (b"\x01b", missing_rows),
    ] {
        for (owner, reference) in rows {
            edits.push((
                term.to_vec(),
                MapEdit::objects(owner.to_vec(), None, Some(reference)),
            ));
        }
    }
    spool.posting_batch(&edits, &mut budget)?;
    let mut root = Root::default();
    let mut context = AttemptContext {
        attempt: test_attempt(workspace),
        objects: &objects,
        maps: &maps,
        custody: &TestCustody,
        scratch: &scratch,
        budget: &mut budget,
    };
    apply_postings(&mut root, &mut context, &spool, None)?;
    // One Small posting block and one canonical dictionary page are the two
    // physical inputs; the four case slots still share the Small child below.
    check_eq!(objects.batch_inputs.load(Ordering::Relaxed), 2);
    let terms = published_terms(&objects, &maps, root.terms, 5, context.budget)?;
    check_eq!(
        terms.get(b"\x01staged".as_slice()).copied(),
        Some(confirmed.into())
    );
    let old = read_posting(b"\x01staged", confirmed, &objects, context.budget)?;
    check_eq!(old.root.inline_map(), Some(prior_map));
    let mut shared = None;
    for (slot, (term, rows)) in [
        (b"\0a".as_slice(), confirmed_rows),
        (b"\0b".as_slice(), missing_rows),
        (b"\x01a".as_slice(), confirmed_rows),
        (b"\x01b".as_slice(), missing_rows),
    ]
    .into_iter()
    .enumerate()
    {
        let value = terms.get(term).copied().ok_or(IndexError::Conflict)?;
        let MapValue::Inline(inline) = value else {
            return Err(IndexError::Conflict);
        };
        let child = inline.single_child()?.ok_or(IndexError::Conflict)?;
        if let Some(expected) = shared {
            check_eq!(child, expected);
        }
        shared = Some(child);
        check_eq!(
            inline.data()?.get(..3),
            Some(
                [
                    3,
                    0,
                    u8::try_from(slot).map_err(|_error| IndexError::ResourceLimited)?
                ]
                .as_slice()
            )
        );
        let posting = read_posting(term, value, &objects, context.budget)?;
        check_eq!(posting.df, 2);
        let small = posting.root.inline_map().ok_or(IndexError::Conflict)?;
        for (ordinal, (owner, reference)) in rows.iter().enumerate() {
            check_eq!(small.entry(ordinal)?, Some((owner.as_slice(), *reference)));
        }
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum SecondChild {
    Valid,
    Changed,
    Deadline,
    WrongTokenTotal,
}

#[derive(Default, Debug)]
struct DirectoryReads {
    visits: usize,
    descriptors: Vec<(Vec<u8>, ObjectRef)>,
    producer_children: Vec<ObjectRef>,
    children: Vec<ObjectRef>,
    global_check: Option<(u64, u64)>,
}

struct DirectoryProbe<'a> {
    maps: &'a CanonicalMaps,
    objects: &'a CanonicalObjects,
    reads: Mutex<DirectoryReads>,
    child_payloads: Arc<ChildPayloadLease>,
    producer_active: AtomicBool,
    directory_staged: AtomicBool,
    second: SecondChild,
}

struct ChildPayloadLease;

impl quanta_index_core::index::MemoryLeaseOwner for ChildPayloadLease {
    fn release(&self, _bytes: u64) {}
}

impl DirectoryProbe<'_> {
    fn read_log(&self) -> Result<std::sync::MutexGuard<'_, DirectoryReads>, IndexError> {
        self.reads
            .lock()
            .map_err(|_error| IndexError::Storage("directory read log".into()))
    }
}

impl PersistentMapPort for DirectoryProbe<'_> {
    fn lookup(
        &self,
        root: MapRoot,
        key: &[u8],
        budget: &mut WorkBudget,
    ) -> Result<Option<MapValue>, IndexError> {
        self.maps.lookup(root, key, budget)
    }

    fn scan(
        &self,
        root: MapRoot,
        after: Option<&[u8]>,
        limit: u32,
        budget: &mut WorkBudget,
    ) -> Result<MapPage, IndexError> {
        let mut page = self.maps.scan(root, after, limit, budget)?;
        if !self.directory_staged.load(Ordering::Relaxed) {
            return Ok(page);
        }
        if matches!(self.second, SecondChild::Changed)
            && after.is_some()
            && let Some((fence, MapValue::Inline(descriptor))) = page.entries.first()
        {
            let original = descriptor.single_child()?.ok_or(IndexError::Conflict)?;
            let mut payload = self
                .objects
                .stored
                .lock()
                .map_err(|_error| IndexError::Storage("fixture block lock".into()))?
                .get(&original)
                .ok_or(IndexError::Conflict)?
                .payload
                .clone();
            let position_byte = payload.last_mut().ok_or(IndexError::ResourceLimited)?;
            *position_byte = position_byte.wrapping_add(1);
            let changed = self.objects.seed(payload, Vec::new())?;
            check!(changed != original);
            // Keep kind, count, and upper fence; replace just the child with
            // a different canonical block, as in the existing reopen oracle.
            page = MapPage::new(
                vec![(
                    fence.clone(),
                    MapValue::inline(descriptor.data()?, Some(changed))?,
                )],
                page.has_more,
                Vec::new(),
                None,
            );
        }
        let mut reads = self.read_log()?;
        for (fence, value) in &page.entries {
            let child = value.single_child()?.ok_or_else(|| {
                IndexError::BackendViolation("directory probe expected a child".into())
            })?;
            reads.descriptors.push((fence.clone(), child));
        }
        Ok(page)
    }

    fn try_visit_scan_pages(
        &self,
        root: MapRoot,
        budget: &mut WorkBudget,
        visitor: &mut dyn FnMut(MapPage, &mut WorkBudget) -> Result<(), IndexError>,
    ) -> Result<bool, IndexError> {
        if !self.directory_staged.load(Ordering::Relaxed) {
            return Ok(false);
        }
        self.read_log()?.visits += 1;
        let mut previous: Option<quanta_index_core::index::MapPageEntry> = None;
        loop {
            check_eq!(Arc::strong_count(&self.child_payloads), 1);
            let after = previous.as_ref().map(|entry| entry.key()).transpose()?;
            let page = self.scan(root, after, 1, budget)?;
            if !page.has_more {
                visitor(page, budget)?;
                return Ok(true);
            }
            previous = page.clone().cursor().next();
            visitor(page, budget)?;
        }
    }

    fn apply(
        &self,
        base: MapRoot,
        edits: &[MapEdit],
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<MapWrite, IndexError> {
        self.maps.apply(base, edits, sink, budget)
    }

    fn apply_batch(
        &self,
        requests: &[MapBatchRequest],
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<MapBatchWrite, IndexError> {
        self.maps.apply_batch(requests, sink, budget)
    }

    fn apply_stream(
        &self,
        bases: &[MapRoot],
        edits: &mut dyn MapEditCursor,
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<StagedMapRoots, IndexError> {
        self.producer_active.store(true, Ordering::Relaxed);
        let result = self.maps.apply_stream(bases, edits, sink, budget);
        self.producer_active.store(false, Ordering::Relaxed);
        let roots = result?;
        // The first and only map application during the inventory batch
        // stages the directory. Subsequent gets belong to post-map diff.
        self.directory_staged.store(true, Ordering::Relaxed);
        Ok(roots)
    }
}

impl ImmutableObjectPort for DirectoryProbe<'_> {
    fn install_batch_memory_bound(
        &self,
        shape: quanta_index_core::index::ports::ObjectInstallMemoryShape,
    ) -> Result<Option<quanta_index_core::index::MemoryCreditLimits>, IndexError> {
        self.objects.install_batch_memory_bound(shape)
    }

    fn identify(
        &self,
        payload: &[u8],
        children: &[ObjectRef],
        budget: &mut WorkBudget,
    ) -> Result<RegisteredObject, IndexError> {
        self.objects.identify(payload, children, budget)
    }

    fn put(
        &self,
        payload: &[u8],
        children: &[ObjectRef],
        budget: &mut WorkBudget,
    ) -> Result<RegisteredObject, IndexError> {
        self.objects.put(payload, children, budget)
    }

    fn install(
        &self,
        payload: &[u8],
        children: &[ObjectRef],
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<RegisteredObject, IndexError> {
        self.objects.install(payload, children, sink, budget)
    }

    fn install_batch(
        &self,
        inputs: &[ObjectBatchInput],
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<RegisteredObjectBatch, IndexError> {
        self.objects.install_batch(inputs, sink, budget)
    }

    fn get(
        &self,
        reference: ObjectRef,
        budget: &mut WorkBudget,
    ) -> Result<StoredObject, IndexError> {
        let object = self.objects.get(reference, budget)?;
        if self.producer_active.load(Ordering::Relaxed) {
            self.read_log()?.producer_children.push(reference);
        }
        if !self.directory_staged.load(Ordering::Relaxed) {
            return Ok(object);
        }
        let mut reads = self.read_log()?;
        reads.children.push(reference);
        let second = reads
            .descriptors
            .get(1)
            .is_some_and(|(_, child)| *child == reference);
        drop(reads);
        if second && matches!(self.second, SecondChild::Deadline) {
            budget.cap_deadline(Instant::now());
            budget.checkpoint()?;
        }
        let custody = MemoryLease::new(1, self.child_payloads.clone());
        Ok(StoredObject::new(
            object.registration.clone(),
            object.payload.clone(),
            Some(custody),
        ))
    }

    fn read_retained(
        &self,
        object: &StoredObject,
        budget: &mut WorkBudget,
    ) -> Result<StoredObject, IndexError> {
        // Retained checked data uses the same logical read/wire/decode charge
        // as the production adapter but does not perform another physical get.
        budget.charge(WorkUsage {
            reads: 1,
            bytes: object.registration.bytes,
            ..WorkUsage::default()
        })?;
        budget.charge(WorkUsage {
            bytes: u64::try_from(object.payload.len())
                .map_err(|_error| IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;
        Ok(object.clone())
    }
}

fn ready_phase<T>(phase: &'static str, result: Result<T, IndexError>) -> Result<T, IndexError> {
    result.map_err(|error| {
        eprintln!("Ready fixture {phase}: {error:?}");
        error
    })
}

// Same source as cold_multiblock_alias_stages_two_selectors_and_reopens_full_directory,
// through the batch boundary that also prepares the diff and completes the owner.
fn multiblock_prepare(
    objects: &CanonicalObjects,
    maps: &CanonicalMaps,
    second: SecondChild,
    reuse_confirmed: bool,
    leaf: bool,
) -> Result<(Option<Rows>, DirectoryReads), IndexError> {
    let fixture = visitor_fixture()?;
    let recipe = fixture.recipe;
    let probe = DirectoryProbe {
        maps,
        objects,
        reads: Mutex::new(DirectoryReads::default()),
        child_payloads: Arc::new(ChildPayloadLease),
        producer_active: AtomicBool::new(false),
        directory_staged: AtomicBool::new(false),
        second,
    };
    probe
        .objects
        .bounded_install_memory
        .store(true, Ordering::Relaxed);
    let body = if leaf {
        format!("{}{}", "solo ".repeat(2050), distinct_terms(256))
    } else {
        format!("{} t256", distinct_terms(256))
    };
    let indexed_tokens = if leaf { 2306 } else { 257 };
    let record = source_record(objects, recipe, &body)?;
    let mut budget = prepare_budget();
    let scratch = TestScratch::default();
    let spool = TextSpool::new(&scratch, &mut budget)?;
    budget.bind_memory_scope(Some(quanta_index_core::index::MemoryScope::new(Arc::new(
        super::super::token_spool_batch::CreditOutputMemoryScope {
            memory: Arc::new(TokenMemory {
                ceiling: 100_000_000,
                ..TokenMemory::default()
            }),
            attempts: Arc::new(AtomicUsize::new(0)),
        },
    ))));
    let stored = probe.objects.get(record, &mut budget)?;
    let new_source = decode::<SourceRecord>(&stored.payload)?;
    let mut context = AttemptContext {
        attempt: test_attempt(fixture.ticket.view.identity.workspace),
        objects: &probe,
        maps: &probe,
        custody: &TestCustody,
        scratch: &scratch,
        budget: &mut budget,
    };
    let mut batch = InventoryBatch::cold(1)?;
    batch.occurrence_writes.pack_owners = true;
    let mut root = Root::default();
    let mut contributions = ContributionWrites::cold()?;
    let mut inventories = InventoryReuse::new(recipe, 64)?;
    // Use the existing fixture's producer setup and stage its two actual
    // selectors before attaching them to a cold batch. The diff still uses the
    // normal bounded occurrence reuse window.
    let mut confirmed = ConfirmedOccurrences::new(if reuse_confirmed { 64 } else { 0 })?;
    let prepared = prepare_inventory(
        &body,
        &EntryMap::default(),
        0,
        &mut context,
        &spool,
        &mut confirmed,
        &mut batch.inventory_edits,
        &mut batch.occurrence_writes,
    )?;
    check!(prepared.changed);
    check_eq!(
        (prepared.term_count, prepared.indexed_tokens),
        (514, indexed_tokens)
    );
    let alias = prepared
        .alias
        .ok_or_else(|| IndexError::BackendViolation("expected a staged cold alias".into()))?;
    check_eq!(alias.terms, 514);
    if !leaf {
        check_eq!(alias.anchors, 2);
    }
    check_eq!(alias.layout, entry_patch::ColdAliasLayout::Directory);
    batch.occurrence_writes.flush(
        &mut context,
        &spool,
        &mut confirmed,
        &mut batch.inventory_edits,
    )?;
    batch.inventory_edits.flush(&spool, context.budget)?;
    let mut selectors = spool.inventory_cursor(1)?;
    for _ in 0..alias.anchors {
        check!(selectors.next_edit(context.budget)?.is_some());
    }
    check!(selectors.next_edit(context.budget)?.is_none());
    batch.rows.push(
        PendingTextChange {
            owner: owner_key(fixture.owner),
            prior_ref: None,
            prior: None,
            next: PendingTextNext::Draft {
                record,
                slot: 0,
                body: TextBodyKey {
                    bytes: new_source.bytes,
                    byte_len: new_source.byte_len,
                },
                tokens: if matches!(second, SecondChild::WrongTokenTotal) {
                    indexed_tokens + 1
                } else {
                    prepared.indexed_tokens
                },
                alias: Some(alias),
                shared: false,
            },
        },
        context.budget,
    )?;
    batch.ready.push(ReadyTextNext::default(), context.budget)?;
    batch.bases.push(EntryMap::default(), context.budget)?;
    batch.counts.push(alias.anchors, context.budget)?;
    batch.directory_receipts.push(None, context.budget)?;
    batch.attempt = Some(context.attempt);
    let before_maps = maps.applies.load(Ordering::Relaxed);
    let trace = Arc::new(PostingTrace::default());
    let diagnostic = DiagnosticContextV1::new(
        DiagnosticCaptureIdV1([32; 16]),
        Arc::new(Tick::default()),
        trace.clone(),
    );
    let parent = diagnostic
        .enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(0))
        .map_err(|error| IndexError::BackendViolation(error.to_string()))?;
    let result = {
        let mut traced_budget = context
            .budget
            .scoped_diagnostic_context(Some(diagnostic.child_context(&parent)));
        let mut traced_context = AttemptContext {
            budget: &mut traced_budget,
            ..context
        };
        batch.flush(
            &mut root,
            &mut traced_context,
            &spool,
            &mut contributions,
            &mut inventories,
        )
    };
    parent
        .finish(if result.is_ok() {
            DiagnosticTerminalV1::Ok
        } else {
            DiagnosticTerminalV1::Error { code: 1 }
        })
        .map_err(|error| IndexError::BackendViolation(error.to_string()))?;
    probe.directory_staged.store(false, Ordering::Relaxed);
    let mut reads = std::mem::take(&mut *probe.read_log()?);
    if result.is_ok() {
        let phase = DiagnosticStageV1::NativeTextInventoryMapApply;
        let step = DiagnosticLaneV1(41);
        reads.global_check = Some((
            trace.stage_work(phase, step, DiagnosticCounterV1::WorkReads)?,
            trace.stage_work(phase, step, DiagnosticCounterV1::WorkBytes)?,
        ));
    }
    let expected = match second {
        SecondChild::Deadline => {
            check_eq!(result, Err(IndexError::ResourceLimited));
            check_eq!(
                context.budget.first_denial(),
                Some(WorkBudgetDenial::Deadline)
            );
            None
        }
        SecondChild::Changed => {
            check!(matches!(result, Err(IndexError::CorruptObject(_))));
            None
        }
        SecondChild::WrongTokenTotal => {
            check!(matches!(
                result,
                Err(IndexError::CorruptObject(message))
                    if message == "native text contribution token count differs from inventory"
            ));
            None
        }
        SecondChild::Valid => {
            if reuse_confirmed {
                ready_phase("first inventory batch", result)?;
            } else {
                result?;
            }
            check_eq!((root.docs, root.indexed_tokens), (1, indexed_tokens));
            if reuse_confirmed {
                ready_phase(
                    "first contribution flush",
                    contributions.flush(&mut context, &spool),
                )?;
            } else {
                contributions.flush(&mut context, &spool)?;
            }
            let owner = owner_key(fixture.owner);
            let mut next_owner = owner;
            next_owner[0] ^= 1;
            if reuse_confirmed {
                // Complete both inventory windows before the single posting
                // seal. Only apply_postings publishes root.owners.
                let installed = objects.block_inputs.load(Ordering::Relaxed);
                check_eq!(installed, 2);
                let second_prepared = ready_phase(
                    "second prepare",
                    prepare_inventory(
                        &body,
                        &EntryMap::default(),
                        0,
                        &mut context,
                        &spool,
                        &mut confirmed,
                        &mut batch.inventory_edits,
                        &mut batch.occurrence_writes,
                    ),
                )?;
                check_eq!(
                    (second_prepared.term_count, second_prepared.indexed_tokens),
                    (514, 257)
                );
                let second_alias = second_prepared.alias.ok_or(IndexError::Conflict)?;
                check_eq!(second_alias.layout, entry_patch::ColdAliasLayout::Directory);
                check_eq!((second_alias.terms, second_alias.anchors), (514, 2));
                check_eq!(second_alias.reference, alias.reference);
                check!(batch.occurrence_writes.pending.is_empty());
                ready_phase(
                    "second inventory edits flush",
                    batch.inventory_edits.flush(&spool, context.budget),
                )?;
                let mut selectors = spool.inventory_cursor(1)?;
                for _ in 0..2 {
                    check!(selectors.next_edit(context.budget)?.is_some());
                }
                check!(selectors.next_edit(context.budget)?.is_none());
                batch.rows.push(
                    PendingTextChange {
                        owner: next_owner,
                        prior_ref: None,
                        prior: None,
                        next: PendingTextNext::Draft {
                            record,
                            slot: 0,
                            body: TextBodyKey {
                                bytes: new_source.bytes,
                                byte_len: new_source.byte_len,
                            },
                            tokens: second_prepared.indexed_tokens,
                            alias: Some(second_alias),
                            shared: false,
                        },
                    },
                    context.budget,
                )?;
                batch.ready.push(ReadyTextNext::default(), context.budget)?;
                batch.bases.push(EntryMap::default(), context.budget)?;
                batch.counts.push(second_alias.anchors, context.budget)?;
                batch.directory_receipts.push(None, context.budget)?;
                batch.attempt = Some(context.attempt);
                ready_phase(
                    "second inventory batch",
                    batch.flush(
                        &mut root,
                        &mut context,
                        &spool,
                        &mut contributions,
                        &mut inventories,
                    ),
                )?;
                check_eq!((root.docs, root.indexed_tokens), (2, 514));
                check_eq!(objects.block_inputs.load(Ordering::Relaxed), installed);
                ready_phase(
                    "second contribution flush",
                    contributions.flush(&mut context, &spool),
                )?;
                let second_reads = std::mem::take(&mut *probe.read_log()?);
                check_eq!(second_reads.producer_children, Vec::<ObjectRef>::new());
                check_eq!(second_reads.descriptors.len(), 2);
                check_eq!(second_reads.children.len(), 2);
                check_eq!(second_reads.children, reads.children);
                probe.directory_staged.store(false, Ordering::Relaxed);
            }
            if reuse_confirmed {
                ready_phase(
                    "final apply_postings",
                    apply_postings(&mut root, &mut context, &spool, None),
                )?;
            } else {
                apply_postings(&mut root, &mut context, &spool, None)?;
            }
            let terms = published_terms(objects, maps, root.terms, 514, context.budget)?;
            let contribution_value = if reuse_confirmed {
                ready_phase(
                    "first owner lookup",
                    probe.maps.lookup(root.owners, &owner, context.budget),
                )?
            } else {
                probe.maps.lookup(root.owners, &owner, context.budget)?
            }
            .ok_or(IndexError::Conflict)?;
            let contribution = if reuse_confirmed {
                ready_phase(
                    "first contribution read",
                    read_contribution(contribution_value, objects, context.budget),
                )?
            } else {
                read_contribution(contribution_value, objects, context.budget)?
            };
            check_eq!(contribution.record, record);
            check_eq!(contribution.indexed_tokens, indexed_tokens);
            let EntryMap::InventoryDirectory(directory) = contribution.inventory else {
                return Err(IndexError::Conflict);
            };
            let descriptors = probe.maps.rows(MapRoot(Some(directory)))?;
            check_eq!(
                descriptors.len(),
                usize::try_from(alias.anchors).map_err(|_error| IndexError::ResourceLimited)?
            );
            if leaf {
                let mut leaf_descriptors = 0;
                let mut packed_descriptors = 0;
                let reopened = probe.objects.reopened()?;
                let mut previous_fence = None;
                for (fence, value) in &descriptors {
                    if let Some(prior) = previous_fence {
                        check!(prior < fence);
                    }
                    previous_fence = Some(fence);
                    let MapValue::Inline(descriptor) = value else {
                        return Err(IndexError::Conflict);
                    };
                    let data = descriptor.data()?;
                    let child = descriptor.single_child()?.ok_or(IndexError::Conflict)?;
                    if data == [2, 0, 1] {
                        leaf_descriptors += 1;
                        check!(
                            reopened
                                .get(child, context.budget)?
                                .payload
                                .starts_with(b"{\"positions\":")
                        );
                    } else {
                        check_eq!(data.first(), Some(&1));
                        packed_descriptors += 1;
                        check!(
                            reopened
                                .get(child, context.budget)?
                                .payload
                                .starts_with(b"QITXOC01")
                        );
                    }
                }
                check_eq!(leaf_descriptors, 2);
                check!(packed_descriptors >= 2);
                let mut cursor = MapEntries::new(&reopened, maps, contribution.inventory);
                let mut reader = crate::native::text_occurrence::OccurrenceReader::new(2)?;
                for case in [CaseMode::Sensitive, CaseMode::Folded] {
                    for (term, positions) in
                        std::iter::once(("solo".to_owned(), (0_u32..2050).collect::<Vec<_>>()))
                            .chain(
                                (0_u32..256).map(|ordinal| {
                                    (format!("t{ordinal:03}"), vec![ordinal + 2050])
                                }),
                            )
                    {
                        let key = stored_term_key(&term, case)?;
                        let entry = cursor.next(context.budget)?.ok_or(IndexError::Conflict)?;
                        let (observed, child) = entry.object_parts()?;
                        check_eq!(observed, &key);
                        let loaded = reader.load(*child, &key, &reopened, context.budget)?;
                        check_eq!(loaded.positions.0.as_slice(), positions.as_slice());
                        check_eq!(
                            terms.get(&key),
                            Some(&MapValue::inline(&owner, Some(*child))?)
                        );
                    }
                }
                check!(cursor.next(context.budget)?.is_none());
                return Ok((Some(terms), reads));
            }
            let fences = [
                stored_term_key("t254", CaseMode::Folded)?,
                stored_term_key("t256", CaseMode::Folded)?,
            ];
            let counts = [[1, 2, 0], [1, 0, 2]];
            let mut blocks = Vec::new();
            let reopened = probe.objects.reopened()?;
            for (((fence, descriptor), expected_fence), count) in
                descriptors.iter().zip(fences).zip(counts)
            {
                check_eq!(fence, &expected_fence);
                let MapValue::Inline(value) = descriptor else {
                    return Err(IndexError::Conflict);
                };
                check_eq!(value.data()?, &count);
                let child = value.single_child()?.ok_or(IndexError::Conflict)?;
                blocks.push(crate::native::text_occurrence::block::CheckedBlock::parse(
                    reopened.get(child, context.budget)?,
                    context.budget,
                )?);
            }
            let mut cursor = MapEntries::new(&reopened, maps, contribution.inventory);
            for case in [CaseMode::Sensitive, CaseMode::Folded] {
                for ordinal in 0_u32..257 {
                    let key = stored_term_key(&format!("t{ordinal:03}"), case)?;
                    let entry = cursor.next(context.budget)?.ok_or(IndexError::Conflict)?;
                    let (observed, child) = entry.object_parts()?;
                    check_eq!(observed, &key);
                    let slot = usize::from(case == CaseMode::Folded && ordinal >= 255);
                    let block = blocks.get(slot).ok_or(IndexError::Conflict)?;
                    check_eq!(*child, block.object().registration.reference);
                    let positions = block.selection(&key, context.budget)?;
                    let bytes = block
                        .object()
                        .payload
                        .get(positions)
                        .ok_or(IndexError::Conflict)?;
                    check_eq!(bytes, &ordinal.to_be_bytes());
                    if !reuse_confirmed {
                        check_eq!(
                            terms.get(&key),
                            Some(&MapValue::inline(&owner, Some(*child))?)
                        );
                    }
                }
            }
            check!(cursor.next(context.budget)?.is_none());
            if reuse_confirmed {
                let second_value = ready_phase(
                    "second owner lookup",
                    maps.lookup(root.owners, &next_owner, context.budget)
                        .and_then(|value| value.ok_or(IndexError::Conflict)),
                )?;
                let second_contribution = ready_phase(
                    "second contribution read",
                    read_contribution(second_value, objects, context.budget),
                )?;
                let EntryMap::InventoryDirectory(second_directory) = second_contribution.inventory
                else {
                    return Err(IndexError::Conflict);
                };
                // CanonicalMaps uses serial test roots. Equal descriptor rows
                // and child identities, not equal map-root bytes, prove reuse.
                check_eq!(
                    maps.rows(MapRoot(Some(second_directory)))?,
                    maps.rows(MapRoot(Some(directory)))?
                );
                check_eq!(second_contribution.indexed_tokens, 257);
                for case in [CaseMode::Sensitive, CaseMode::Folded] {
                    for ordinal in 0_u32..257 {
                        let key = stored_term_key(&format!("t{ordinal:03}"), case)?;
                        let value = *terms.get(&key).ok_or(IndexError::Conflict)?;
                        let posting = ready_phase(
                            "final posting read",
                            read_posting(&key, value, objects, context.budget),
                        )?;
                        check_eq!(posting.df, 2);
                        let small = posting.root.inline_map().ok_or(IndexError::Conflict)?;
                        let block_slot = usize::from(case == CaseMode::Folded && ordinal >= 255);
                        let expected_child = blocks
                            .get(block_slot)
                            .ok_or(IndexError::Conflict)?
                            .object()
                            .registration
                            .reference;
                        for (slot, expected_owner) in [owner, next_owner].iter().enumerate() {
                            check_eq!(
                                small.entry(slot)?,
                                Some((expected_owner.as_slice(), expected_child))
                            );
                        }
                    }
                }
            }
            Some(terms)
        }
    };
    if expected.is_none() {
        check_eq!(root, Root::default());
        check_eq!(contributions.pending.len(), 0);
        check_eq!(
            maps.applies
                .load(Ordering::Relaxed)
                .checked_sub(before_maps),
            Some(1)
        );
        check!(matches!(
            spool.top_cursor().next_edit(&mut prepare_budget()),
            Err(IndexError::BackendViolation(_))
        ));
    }
    Ok((expected, reads))
}

#[test]
fn cold_multiblock_alias_prepare_visits_each_descriptor_and_child_once() -> Result<(), IndexError> {
    let (prepared, reads) = multiblock_prepare(
        &CanonicalObjects::default(),
        &CanonicalMaps::default(),
        SecondChild::Valid,
        false,
        false,
    )?;
    check!(prepared.is_some());
    check_eq!(reads.visits, 1);
    check_eq!(reads.descriptors.len(), 2);
    // One SQL query, both original row reads, and two 5-byte order keys.
    // Each 38-byte descriptor wire retains its existing edit digest charge.
    check_eq!(
        reads.global_check,
        Some((2, 4_096 + 2 * (5 + 74 + 38 + 8 + 5)))
    );
    check_eq!(reads.producer_children, Vec::<ObjectRef>::new());
    check_eq!(reads.children.len(), 2);
    check_eq!(
        reads.children,
        reads
            .descriptors
            .iter()
            .map(|(_, child)| *child)
            .collect::<Vec<_>>()
    );
    Ok(())
}

#[test]
fn cold_multiblock_alias_ready_reuse_keeps_confirmed_children_and_full_diff()
-> Result<(), IndexError> {
    let objects = CanonicalObjects::default();
    let maps = CanonicalMaps::default();
    let (prepared, reads) = multiblock_prepare(&objects, &maps, SecondChild::Valid, true, false)?;
    check!(prepared.is_some());
    check_eq!(reads.producer_children, Vec::<ObjectRef>::new());
    check_eq!(reads.descriptors.len(), 2);
    check_eq!(reads.children.len(), 2);
    Ok(())
}

#[test]
fn cold_multiblock_alias_leaf_directory_preserves_positions_and_reads_once()
-> Result<(), IndexError> {
    let (prepared, reads) = multiblock_prepare(
        &CanonicalObjects::default(),
        &CanonicalMaps::default(),
        SecondChild::Valid,
        false,
        true,
    )?;
    check!(prepared.is_some());
    check_eq!(reads.producer_children, Vec::<ObjectRef>::new());
    check_eq!(reads.children.len(), reads.descriptors.len());
    check_eq!(
        reads.children,
        reads
            .descriptors
            .iter()
            .map(|(_, child)| *child)
            .collect::<Vec<_>>()
    );
    Ok(())
}

#[test]
fn cold_multiblock_alias_late_child_failure_leaves_no_owner_and_fresh_retry_matches()
-> Result<(), IndexError> {
    let (expected, _) = multiblock_prepare(
        &CanonicalObjects::default(),
        &CanonicalMaps::default(),
        SecondChild::Valid,
        false,
        false,
    )?;
    for fault in [
        SecondChild::Changed,
        SecondChild::Deadline,
        SecondChild::WrongTokenTotal,
    ] {
        let objects = CanonicalObjects::default();
        let maps = CanonicalMaps::default();
        let (failed, reads) = multiblock_prepare(&objects, &maps, fault, false, false)?;
        check!(failed.is_none());
        check_eq!(reads.visits, 1);
        check_eq!(reads.descriptors.len(), 2);
        check_eq!(
            reads.children,
            reads
                .descriptors
                .iter()
                .map(|(_, child)| *child)
                .collect::<Vec<_>>()
        );
        // Reuse the store containing the failed attempt's provisional data;
        // renew the budget, scratch, and injected port fault.
        let (retry, _) = multiblock_prepare(&objects, &maps, SecondChild::Valid, false, false)?;
        check_eq!(retry, expected);
    }
    Ok(())
}
