//! Independent Literal source-read and persisted-seal failure oracles.

use super::*;
use quanta_index_core::index::{
    AttemptRef, MapBatchRequest, MapBatchWrite, MapEdit, MapPage, MapValue, MapWrite,
    MemoryCreditLimits, MemoryLifetime, ObjectBatchInput, ObjectRef, RegisteredObject,
    RegisteredObjectBatch, SourceRecordOwner, StagedMapRoots, StoredObject,
    ports::{
        MapDiffRange, MapDiffVisitor, MapEditCursor, ObjectInstallMemoryShape, ObjectWriteSink,
    },
};
use std::collections::BTreeMap;
use std::sync::{Mutex, atomic::AtomicBool};

struct DirectoryPageFaultMaps {
    inner: Arc<FileObjectStore>,
    prior_directory: std::sync::OnceLock<MapRoot>,
    fail_second_page: bool,
    pages: AtomicUsize,
    continuation_pages: AtomicUsize,
    failures: AtomicUsize,
}

impl DirectoryPageFaultMaps {
    fn new(inner: Arc<FileObjectStore>, fail_second_page: bool) -> Self {
        Self {
            inner,
            prior_directory: std::sync::OnceLock::new(),
            fail_second_page,
            pages: AtomicUsize::new(0),
            continuation_pages: AtomicUsize::new(0),
            failures: AtomicUsize::new(0),
        }
    }
}

impl PersistentMapPort for DirectoryPageFaultMaps {
    fn lookup(
        &self,
        root: MapRoot,
        key: &[u8],
        budget: &mut WorkBudget,
    ) -> Result<Option<MapValue>, IndexError> {
        self.inner.lookup(root, key, budget)
    }

    fn scan(
        &self,
        root: MapRoot,
        after: Option<&[u8]>,
        limit: u32,
        budget: &mut WorkBudget,
    ) -> Result<MapPage, IndexError> {
        self.inner.scan(root, after, limit, budget)
    }

    fn last_entry(&self, root: MapRoot, budget: &mut WorkBudget) -> Result<MapPage, IndexError> {
        self.inner.last_entry(root, budget)
    }

    fn visit_diff(
        &self,
        before: MapRoot,
        after: MapRoot,
        range: MapDiffRange<'_>,
        visitor: &mut dyn MapDiffVisitor,
        budget: &mut WorkBudget,
    ) -> Result<bool, IndexError> {
        if self.prior_directory.get() == Some(&before) && before != after {
            let _previous = self.pages.fetch_add(1, Ordering::Relaxed);
            if range.after.is_some() {
                let prior = self.continuation_pages.fetch_add(1, Ordering::Relaxed);
                if self.fail_second_page && prior == 0 {
                    let _previous = self.failures.fetch_add(1, Ordering::Relaxed);
                    return Err(IndexError::BackendViolation(
                        "injected second Literal directory diff page".into(),
                    ));
                }
            }
        }
        self.inner.visit_diff(before, after, range, visitor, budget)
    }

    fn apply(
        &self,
        base: MapRoot,
        edits: &[MapEdit],
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<MapWrite, IndexError> {
        self.inner.apply(base, edits, sink, budget)
    }

    fn apply_batch(
        &self,
        requests: &[MapBatchRequest],
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<MapBatchWrite, IndexError> {
        self.inner.apply_batch(requests, sink, budget)
    }

    fn apply_stream(
        &self,
        bases: &[MapRoot],
        edits: &mut dyn MapEditCursor,
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<StagedMapRoots, IndexError> {
        self.inner.apply_stream(bases, edits, sink, budget)
    }
}

struct CountedObjects {
    inner: Arc<FileObjectStore>,
    source_body: Mutex<Option<ObjectRef>>,
    reads: AtomicUsize,
    adapter_active: AtomicBool,
    verifying: AtomicBool,
    fault: AtomicUsize,
    faults_observed: AtomicUsize,
    adapter_errors: AtomicUsize,
}

impl CountedObjects {
    fn new(inner: Arc<FileObjectStore>, fault: usize) -> Self {
        Self {
            inner,
            source_body: Mutex::new(None),
            reads: AtomicUsize::new(0),
            adapter_active: AtomicBool::new(false),
            verifying: AtomicBool::new(false),
            fault: AtomicUsize::new(fault),
            faults_observed: AtomicUsize::new(0),
            adapter_errors: AtomicUsize::new(0),
        }
    }

    fn watch(&self, body: ObjectRef) -> TestResult {
        *self
            .source_body
            .lock()
            .map_err(|_| "source body mutex poisoned")? = Some(body);
        self.reads.store(0, Ordering::Relaxed);
        Ok(())
    }
}

impl ImmutableObjectPort for CountedObjects {
    fn install_batch_memory_bound(
        &self,
        shape: ObjectInstallMemoryShape,
    ) -> Result<Option<MemoryCreditLimits>, IndexError> {
        self.inner.install_batch_memory_bound(shape)
    }

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
    ) -> Result<RegisteredObjectBatch, IndexError> {
        self.inner.install_batch(inputs, sink, budget)
    }

    fn try_prepare_leaf_install_run(
        &self,
        attempt: AttemptRef,
        maximum_payload: usize,
        packet_bytes: usize,
        budget: &mut WorkBudget,
    ) -> Result<
        Option<Box<dyn quanta_index_core::index::object_install::PreparedObjectInstallRun + '_>>,
        IndexError,
    > {
        self.inner
            .try_prepare_leaf_install_run(attempt, maximum_payload, packet_bytes, budget)
    }

    fn get(
        &self,
        reference: ObjectRef,
        budget: &mut WorkBudget,
    ) -> Result<StoredObject, IndexError> {
        if self.adapter_active.load(Ordering::Relaxed)
            && self
                .source_body
                .lock()
                .map_err(|_| IndexError::BackendViolation("source body mutex poisoned".into()))?
                .is_some_and(|body| body == reference)
        {
            let _previous_reads = self.reads.fetch_add(1, Ordering::Relaxed);
        }
        let stored = self.inner.get(reference, budget)?;
        if self.adapter_active.load(Ordering::Relaxed)
            && self.fault.load(Ordering::Relaxed) == 1
            && stored.payload.starts_with(b"QILB")
        {
            // Keep the framing intact while changing a decoded source key.
            // A hostile backend must never turn this into a published member.
            let memory = budget.reserve_memory(
                StoredObject::allocation_bytes(stored.payload.len(), 0)?,
                MemoryLifetime::Temporary,
            )?;
            let mut bytes = stored.payload.clone();
            *bytes.get_mut(14).ok_or_else(|| {
                IndexError::BackendViolation("test block has no first key".into())
            })? = 0;
            let _previous_faults = self.faults_observed.fetch_add(1, Ordering::Relaxed);
            return Ok(StoredObject::new(
                stored.registration.clone(),
                bytes,
                memory,
            ));
        }
        if self.verifying.load(Ordering::Relaxed)
            && self.fault.load(Ordering::Relaxed) == 2
            && stored.payload.starts_with(b"QLITBLK2")
        {
            let _previous_faults = self.faults_observed.fetch_add(1, Ordering::Relaxed);
            return Err(IndexError::BackendViolation(
                "test persisted Literal root read failed during seal".into(),
            ));
        }
        Ok(stored)
    }
}

struct ObservedLiteral(Arc<CountedObjects>);

impl IndexBuildPort for ObservedLiteral {
    type Delta = SourceDelta;
    type Plan = <NativeLiteralIndex as IndexBuildPort>::Plan;
    type Candidate = <NativeLiteralIndex as IndexBuildPort>::Candidate;

    fn resource_demand(&self) -> quanta_index_core::index::effort::ResourceNeed {
        NativeLiteralIndex.resource_demand()
    }

    fn validate_instance(&self, instance: &InstanceSpec) -> Result<(), IndexError> {
        NativeLiteralIndex.validate_instance(instance)
    }

    fn plan(
        &self,
        base: BaseMember,
        input: DeltaInput<SourceDelta>,
        context: &PlanContext,
        budget: &mut WorkBudget,
    ) -> Result<PlanDecision<Self::Plan>, IndexError> {
        NativeLiteralIndex.plan(base, input, context, budget)
    }

    fn prepare<'a>(
        &'a self,
        plan: Self::Plan,
        context: &'a mut AttemptContext<'_>,
    ) -> IndexFuture<'a, Self::Candidate> {
        Box::pin(async move {
            self.0.adapter_active.store(true, Ordering::Relaxed);
            let result = NativeLiteralIndex.prepare(plan, context).await;
            if result.is_err() {
                let _previous_errors = self.0.adapter_errors.fetch_add(1, Ordering::Relaxed);
            }
            self.0.adapter_active.store(false, Ordering::Relaxed);
            result
        })
    }

    fn verify_and_seal<'a>(
        &'a self,
        mut candidate: Self::Candidate,
        context: &'a mut AttemptContext<'_>,
    ) -> IndexFuture<'a, MemberBinding> {
        Box::pin(async move {
            self.0.adapter_active.store(true, Ordering::Relaxed);
            self.0.verifying.store(true, Ordering::Relaxed);
            if self.0.fault.load(Ordering::Relaxed) == 3 {
                candidate.binding.input.policy = Digest([222; 32]);
                let _previous_faults = self.0.faults_observed.fetch_add(1, Ordering::Relaxed);
            }
            let result = NativeLiteralIndex.verify_and_seal(candidate, context).await;
            if result.is_err() {
                let _previous_errors = self.0.adapter_errors.fetch_add(1, Ordering::Relaxed);
            }
            self.0.verifying.store(false, Ordering::Relaxed);
            self.0.adapter_active.store(false, Ordering::Relaxed);
            result
        })
    }
}

#[test]
fn cold_literal_prepare_and_seal_derives_source_body_once() -> TestResult {
    check_literal_seal(0)
}

#[test]
fn literal_seal_refuses_changed_persisted_child_without_publication() -> TestResult {
    check_literal_seal(1)
}

#[test]
fn literal_seal_refuses_late_root_read_failure_without_publication() -> TestResult {
    check_literal_seal(2)
}

#[test]
fn literal_seal_refuses_forged_candidate_binding_without_publication() -> TestResult {
    check_literal_seal(3)
}

#[test]
fn warm_literal_patch_seal_refuses_late_root_read_without_publication() -> TestResult {
    check_literal_seal_with_patch(2)
}

fn check_literal_seal(fault: usize) -> TestResult {
    check_literal_seal_case(fault, false)
}

fn check_literal_seal_with_patch(fault: usize) -> TestResult {
    check_literal_seal_case(fault, true)
}

fn check_literal_seal_case(fault: usize, warm_patch: bool) -> TestResult {
    let root = tempfile::tempdir()?;
    let objects = Arc::new(FileObjectStore::open(root.path().join("objects"))?);
    let counted = Arc::new(CountedObjects::new(
        objects.clone(),
        if warm_patch { 0 } else { fault },
    ));
    let catalog = Arc::new(SqliteCatalog::open(
        root.path(),
        Duration::from_millis(100),
    )?);
    let [literal, _, _] = specs()?;
    let mut registry = AdapterRegistry::new();
    registry.register(
        descriptor(&literal),
        ObservedLiteral(counted.clone()),
        |delta| Ok(delta.clone()),
    )?;
    let services = KernelServices::new(
        counted.clone(),
        objects.clone(),
        catalog,
        Arc::new(FileScratchSpace::open_standalone(
            root.path().join("scratch"),
            16 * 1024 * 1024 * 1024,
        )?),
        Duration::from_secs(120),
    )?;
    let engine = Kernel::new(services, registry);
    let genesis = engine.create_workspace(workspace(), 1, &mut budget()?)?;
    let ingress = engine.begin_input(operation(1))?;
    let records = engine.stage_sources(
        ingress,
        vec![SourceInput {
            path: "src/one.rs".into(),
            bytes: b"Needle cafe Needle".to_vec(),
            text: SourceTextAdmission::Utf8 { policy: ADMISSION },
            language: None,
        }],
        &mut budget()?,
    )?;
    let record_ref = records
        .iter()
        .next()
        .copied()
        .ok_or("missing staged source record")?;
    let mut read_budget = budget()?;
    let stored = objects.get(record_ref, &mut read_budget)?;
    let record = SourceRecordOwner::decode_json(
        &stored.payload,
        &mut read_budget,
        MemoryLifetime::Temporary,
    )?;
    let body_ref = record.bytes;
    let mutations = engine.stage_mutations(
        ingress,
        [Ok(OwnerMutation {
            owner: owner(1),
            expected: None,
            next: Some(record_ref),
        })],
        &mut budget()?,
    )?;
    let before = engine.pin_view(ViewSelection::Exact(genesis), &budget()?)?;
    let mut transition = SourceTransition {
        expected_source: before.view().source,
        owners: mutations,
        events: Vec::new(),
    };
    transition.events.push(SourceEvent {
        producer: operation(1).producer,
        stream: operation(1).stream,
        stream_epoch: 1,
        predecessor: 0,
        sequence: 1,
        payload: transition.owner_digest()?,
    });
    before.release()?;
    let source_only = published(block_on(engine.submit(
        UpdateRequest {
            operation: operation(1),
            expected_view: genesis,
            kind: UpdateKind::SourceAdvance(transition),
        },
        &mut budget()?,
    ))?)?;
    counted.watch(body_ref)?;
    let (base, sequence, outcome) = if warm_patch {
        let installed = published(reconfigure(&engine, source_only, 2, vec![literal])?)?;
        counted.fault.store(fault, Ordering::Relaxed);
        let key = operation(3);
        let ingress = engine.begin_input(key)?;
        let next = engine.stage_sources(
            ingress,
            vec![SourceInput {
                path: "src/one.rs".into(),
                bytes: b"Needle cafe Changed".to_vec(),
                text: SourceTextAdmission::Utf8 { policy: ADMISSION },
                language: None,
            }],
            &mut budget()?,
        )?;
        let next_ref = next
            .iter()
            .next()
            .copied()
            .ok_or("missing Patch source record")?;
        let mutations = engine.stage_mutations(
            ingress,
            [Ok(OwnerMutation {
                owner: owner(1),
                expected: Some(record_ref),
                next: Some(next_ref),
            })],
            &mut budget()?,
        )?;
        let pinned = engine.pin_view(ViewSelection::Exact(installed), &budget()?)?;
        let mut transition = SourceTransition {
            expected_source: pinned.view().source,
            owners: mutations,
            events: Vec::new(),
        };
        transition.events.push(SourceEvent {
            producer: key.producer,
            stream: key.stream,
            stream_epoch: key.stream_epoch,
            predecessor: 1,
            sequence: 2,
            payload: transition.owner_digest()?,
        });
        pinned.release()?;
        let outcome: TestResult<SubmitOutcome> = block_on(engine.submit(
            UpdateRequest {
                operation: key,
                expected_view: installed,
                kind: UpdateKind::SourceAdvance(transition),
            },
            &mut budget()?,
        ))
        .map_err(Into::into);
        (installed, 3, outcome)
    } else {
        (
            source_only,
            2,
            reconfigure(&engine, source_only, 2, vec![literal]),
        )
    };
    if fault != 0 {
        assert!(counted.faults_observed.load(Ordering::Relaxed) > 0);
        assert_eq!(counted.adapter_errors.load(Ordering::Relaxed), 1);
        // Corruption retains a retryable attempt under the existing Kernel
        // contract. A parked attempt is not a successful publication.
        match outcome {
            Ok(SubmitOutcome::Terminal(OperationStatus::Refused(_))) | Err(_) => {}
            Ok(SubmitOutcome::Accepted(attempt)) => {
                assert_eq!(attempt.operation, operation(sequence));
                let inspection = budget()?;
                assert_eq!(
                    engine.status(
                        operation(sequence),
                        quanta_index_core::index::ports::OperationInspectAccess::Request(
                            &inspection,
                        ),
                    )?,
                    OperationStatus::Pending(attempt),
                );
            }
            Ok(SubmitOutcome::Unknown(key)) => {
                assert_eq!(key, operation(sequence));
                let inspection = budget()?;
                let status = engine.status(
                    key,
                    quanta_index_core::index::ports::OperationInspectAccess::Request(&inspection),
                )?;
                assert!(
                    matches!(status, OperationStatus::Pending(attempt) if attempt.operation == key)
                );
            }
            other => panic!("failed seal produced an unexpected outcome: {other:?}"),
        }
        let latest = engine.pin_view(
            ViewSelection::Latest {
                workspace: workspace(),
                epoch: 1,
            },
            &budget()?,
        )?;
        assert_eq!(latest.view().identity, base, "failed seal moved the head");
        latest.release()?;
        return Ok(());
    }
    let _literal_view = published(outcome?)?;
    assert_eq!(
        counted.reads.load(Ordering::Relaxed),
        1,
        "one source body must be derived once across Literal prepare and seal"
    );
    Ok(())
}

fn multipage_literal_body(seed: u64) -> Vec<u8> {
    let mut state = seed;
    let mut body = Vec::with_capacity(25_000);
    for _ in 0..25_000 {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        body.push(0x80 | (state.to_le_bytes()[0] & 0x7f));
    }
    body
}

fn multipage_literal_owner(id: u8) -> OwnerKey {
    // Match the proven large-directory fixture: a different first stable byte
    // prevents the packed rows for adjacent owners from sharing that prefix.
    OwnerKey {
        producer: ProducerId([5; 16]),
        stable_id: StableOwnerId([id; 16]),
    }
}

fn literal_seal_directory_rows(
    engine: &Kernel<SqliteCatalog>,
    objects: &FileObjectStore,
    view: ViewRef,
) -> TestResult<(MapRoot, BTreeMap<Vec<u8>, ObjectRef>)> {
    let ticket = engine.pin_view(ViewSelection::Exact(view), &budget()?)?;
    let [literal, _, _] = specs()?;
    let member = binding(objects, ticket.record(), literal.instance)?;
    let artifact = objects.get(member.artifact, &mut budget()?)?;
    let [_sources, directory] = artifact.registration.children.as_slice() else {
        return Err("expected source and block roots on nonempty Literal member".into());
    };
    let root = MapRoot(Some(*directory));
    let mut rows = BTreeMap::new();
    let mut after = None;
    loop {
        let page = objects.scan(root, after.as_deref(), 64, &mut budget()?)?;
        if page.entries.is_empty() && page.has_more {
            return Err("Literal directory returned an empty continuation".into());
        }
        for (key, value) in &page.entries {
            if rows.insert(key.clone(), *value.as_object_ref()?).is_some() {
                return Err("Literal directory repeated a descriptor".into());
            }
        }
        if !page.has_more {
            break;
        }
        after = Some(
            page.entries
                .last()
                .ok_or("Literal directory continuation lacks a fence")?
                .0
                .clone(),
        );
    }
    ticket.release()?;
    Ok((root, rows))
}

fn check_literal_multipage_seal(fail_second_page: bool) -> TestResult {
    let root = tempfile::tempdir()?;
    let objects = Arc::new(FileObjectStore::open(root.path().join("objects"))?);
    let maps = Arc::new(DirectoryPageFaultMaps::new(
        objects.clone(),
        fail_second_page,
    ));
    let catalog = Arc::new(SqliteCatalog::open(
        root.path(),
        Duration::from_millis(100),
    )?);
    let [literal, _, _] = specs()?;
    let mut registry = AdapterRegistry::new();
    registry.register(descriptor(&literal), NativeLiteralIndex, |delta| {
        Ok(delta.clone())
    })?;
    let services = KernelServices::new(
        objects.clone(),
        maps.clone(),
        catalog,
        Arc::new(FileScratchSpace::open_standalone(
            root.path().join("scratch"),
            16 * 1024 * 1024 * 1024,
        )?),
        Duration::from_secs(120),
    )?;
    let engine = Kernel::new(services, registry);
    let genesis = engine.create_workspace(workspace(), 1, &mut budget()?)?;
    let old_body = multipage_literal_body(0x9e37_79b9_7f4a_7c15);
    let next_body = multipage_literal_body(0xd1b5_4a32_d192_ed03);
    let initial = engine.begin_input(operation(1))?;
    let old_records = engine.stage_sources(
        initial,
        (1..=8)
            .map(|id| SourceInput {
                path: format!("src/{id:02}.bin"),
                bytes: old_body.clone(),
                text: SourceTextAdmission::Excluded,
                language: None,
            })
            .collect(),
        &mut budget()?,
    )?;
    let mutations = engine.stage_mutations(
        initial,
        (1..=8)
            .zip(old_records.iter().copied())
            .map(|(id, reference)| {
                Ok(OwnerMutation {
                    owner: multipage_literal_owner(id),
                    expected: None,
                    next: Some(reference),
                })
            }),
        &mut budget()?,
    )?;
    let prior = engine.pin_view(ViewSelection::Exact(genesis), &budget()?)?;
    let mut transition = SourceTransition {
        expected_source: prior.view().source,
        owners: mutations,
        events: Vec::new(),
    };
    transition.events.push(SourceEvent {
        producer: operation(1).producer,
        stream: operation(1).stream,
        stream_epoch: 1,
        predecessor: 0,
        sequence: 1,
        payload: transition.owner_digest()?,
    });
    prior.release()?;
    let source_only = published(block_on(engine.submit(
        UpdateRequest {
            operation: operation(1),
            expected_view: genesis,
            kind: UpdateKind::SourceAdvance(transition),
        },
        &mut budget()?,
    ))?)?;
    let literal_instance = literal.instance;
    let installed = published(reconfigure(&engine, source_only, 2, vec![literal])?)?;
    let (prior_directory, before_rows) = literal_seal_directory_rows(&engine, &objects, installed)?;
    assert!(
        before_rows.len() > 64,
        "fixture needs more than one directory page"
    );
    assert_literal_matches(&engine, &objects, installed, &[0xff; 3], &[])?;

    let key = operation(3);
    let ingress = engine.begin_input(key)?;
    let mut inputs = (1..=8)
        .map(|id| SourceInput {
            path: format!("src/{id:02}.bin"),
            bytes: next_body.clone(),
            text: SourceTextAdmission::Excluded,
            language: None,
        })
        .collect::<Vec<_>>();
    inputs.push(SourceInput {
        path: "src/last.bin".into(),
        bytes: vec![0xff; 3],
        text: SourceTextAdmission::Excluded,
        language: None,
    });
    let next_records = engine.stage_sources(ingress, inputs, &mut budget()?)?;
    let mutations = engine.stage_mutations(
        ingress,
        (1..=8)
            .zip(
                old_records
                    .iter()
                    .copied()
                    .zip(next_records.iter().copied()),
            )
            .map(|(id, (old, next))| {
                Ok(OwnerMutation {
                    owner: multipage_literal_owner(id),
                    expected: Some(old),
                    next: Some(next),
                })
            })
            .chain([Ok(OwnerMutation {
                owner: multipage_literal_owner(9),
                expected: None,
                next: Some(*next_records.get(8).ok_or("last record missing")?),
            })]),
        &mut budget()?,
    )?;
    let old_ticket = engine.pin_view(ViewSelection::Exact(installed), &budget()?)?;
    let mut transition = SourceTransition {
        expected_source: old_ticket.view().source,
        owners: mutations,
        events: Vec::new(),
    };
    transition.events.push(SourceEvent {
        producer: key.producer,
        stream: key.stream,
        stream_epoch: key.stream_epoch,
        predecessor: 1,
        sequence: 2,
        payload: transition.owner_digest()?,
    });
    maps.prior_directory
        .set(prior_directory)
        .map_err(|_| "Literal directory fault already configured")?;
    let outcome = block_on(engine.submit(
        UpdateRequest {
            operation: key,
            expected_view: installed,
            kind: UpdateKind::SourceAdvance(transition),
        },
        &mut budget()?,
    ));
    assert!(maps.pages.load(Ordering::Relaxed) >= 2);
    assert!(maps.continuation_pages.load(Ordering::Relaxed) >= 1);
    if fail_second_page {
        assert_eq!(maps.failures.load(Ordering::Relaxed), 1);
        assert_eq!(
            outcome?,
            SubmitOutcome::Terminal(OperationStatus::Refused(Refusal::BackendViolation))
        );
        let status = engine.status(
            key,
            quanta_index_core::index::ports::OperationInspectAccess::Request(&budget()?),
        )?;
        assert_eq!(status, OperationStatus::Refused(Refusal::BackendViolation));
        let latest = engine.pin_view(
            ViewSelection::Latest {
                workspace: workspace(),
                epoch: 1,
            },
            &budget()?,
        )?;
        assert_eq!(latest.view().identity, installed);
        latest.release()?;
        assert_eq!(old_ticket.view().identity, installed);
        let old_member = binding(&objects, old_ticket.record(), literal_instance)?;
        let old_page = NativeLiteralIndex.search(
            &old_member,
            LiteralQuery {
                literal: &old_body[..3],
                after: None,
                limit: 16,
            },
            LiteralReadContext {
                ticket: old_ticket.record(),
                input: old_member.input,
                objects: objects.as_ref(),
                maps: objects.as_ref(),
                budget: &mut budget()?,
            },
        )?;
        assert_eq!(
            old_page
                .hits
                .iter()
                .map(|hit| hit.owner)
                .collect::<Vec<_>>(),
            (1..=8).map(multipage_literal_owner).collect::<Vec<_>>()
        );
        let absent = NativeLiteralIndex.search(
            &old_member,
            LiteralQuery {
                literal: &[0xff; 3],
                after: None,
                limit: 16,
            },
            LiteralReadContext {
                ticket: old_ticket.record(),
                input: old_member.input,
                objects: objects.as_ref(),
                maps: objects.as_ref(),
                budget: &mut budget()?,
            },
        )?;
        assert!(absent.hits.is_empty());
    } else {
        assert_eq!(maps.failures.load(Ordering::Relaxed), 0);
        let published = published(outcome?)?;
        let (_, after_rows) = literal_seal_directory_rows(&engine, &objects, published)?;
        let changed = before_rows
            .iter()
            .filter(|(key, value)| after_rows.get(*key) != Some(*value))
            .count()
            + after_rows
                .keys()
                .filter(|key| !before_rows.contains_key(*key))
                .count();
        assert!(
            changed > 64,
            "fixture needs at least 65 changed descriptors"
        );
        let mut last_key = vec![0xff; 3];
        last_key.extend_from_slice(&multipage_literal_owner(9).producer.0);
        last_key.extend_from_slice(&multipage_literal_owner(9).stable_id.0);
        assert_eq!(
            after_rows.last_key_value().map(|(key, _)| key),
            Some(&last_key)
        );
        assert_literal_matches(
            &engine,
            &objects,
            published,
            &[0xff; 3],
            &[multipage_literal_owner(9)],
        )?;
        assert_literal_matches(&engine, &objects, installed, &[0xff; 3], &[])?;
    }
    old_ticket.release()?;
    Ok(())
}

#[test]
fn literal_patch_seals_more_than_one_directory_diff_page_through_last_change() -> TestResult {
    check_literal_multipage_seal(false)
}

#[test]
fn literal_patch_second_directory_diff_page_failure_preserves_old_view() -> TestResult {
    check_literal_multipage_seal(true)
}
