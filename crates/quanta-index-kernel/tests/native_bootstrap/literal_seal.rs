//! Independent cold Literal source-read and persisted-seal failure oracles.

use super::*;
use quanta_index_core::index::{
    AttemptRef, MemoryCreditLimits, MemoryLifetime, ObjectBatchInput, ObjectRef, RegisteredObject,
    RegisteredObjectBatch, SourceRecordOwner, StoredObject,
    ports::{ObjectInstallMemoryShape, ObjectWriteSink},
};
use std::sync::{Mutex, atomic::AtomicBool};

struct CountedObjects {
    inner: Arc<FileObjectStore>,
    source_body: Mutex<Option<ObjectRef>>,
    reads: AtomicUsize,
    adapter_active: AtomicBool,
    verifying: AtomicBool,
    fault: usize,
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
            fault,
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
            && self.fault == 1
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
            && self.fault == 2
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
            if self.0.fault == 3 {
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

fn check_literal_seal(fault: usize) -> TestResult {
    let root = tempfile::tempdir()?;
    let objects = Arc::new(FileObjectStore::open(root.path().join("objects"))?);
    let counted = Arc::new(CountedObjects::new(objects.clone(), fault));
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
    let record =
        SourceRecordOwner::decode_json(&stored.payload, &read_budget, MemoryLifetime::Temporary)?;
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
    let outcome = reconfigure(&engine, source_only, 2, vec![literal]);
    if fault != 0 {
        assert!(counted.faults_observed.load(Ordering::Relaxed) > 0);
        assert_eq!(counted.adapter_errors.load(Ordering::Relaxed), 1);
        // Corruption retains a retryable attempt under the existing Kernel
        // contract. A parked attempt is not a successful publication.
        match outcome {
            Ok(SubmitOutcome::Terminal(OperationStatus::Refused(_))) | Err(_) => {}
            Ok(SubmitOutcome::Accepted(attempt)) => {
                assert_eq!(attempt.operation, operation(2));
                let inspection = budget()?;
                assert_eq!(
                    engine.status(
                        operation(2),
                        quanta_index_core::index::ports::OperationInspectAccess::Request(
                            &inspection,
                        ),
                    )?,
                    OperationStatus::Pending(attempt),
                );
            }
            Ok(SubmitOutcome::Unknown(key)) => {
                assert_eq!(key, operation(2));
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
        assert_eq!(
            latest.view().identity,
            source_only,
            "failed seal moved the head"
        );
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
