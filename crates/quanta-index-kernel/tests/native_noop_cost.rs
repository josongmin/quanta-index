//! Frontier-only source updates must not prepare each unchanged async owner index.
#![expect(
    clippy::disallowed_methods,
    reason = "test-only synchronous driver executes the Kernel future"
)]
#![expect(
    clippy::panic_in_result_fn,
    reason = "integration assertions are independent behavior oracles"
)]

use std::{
    error::Error,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use futures::executor::block_on;
use quanta_index_catalog::SqliteCatalog;
use quanta_index_core::RequestBudgetV1;
use quanta_index_core::index::{
    CollectionId, CoverageBinding, CoverageStatus, Digest, GroupActivation, IndexInstanceId,
    IndexKind, IndexProfile, InputDomain, InstanceSpec, OperationKey, OperationStatus, OwnerKey,
    OwnerMutation, ProducerId, PublicationGroupId, SchemaId, ScratchAllocation, ScratchRequest,
    ScratchSpacePort, SlotId, SourceCut, SourceEvent, SourceRecord, SourceTextAdmission,
    SourceTransition, StableOwnerId, StorageFormatId, StreamId, SubmitOutcome, TenantId,
    UpdateKind, UpdateRequest, ViewRef, ViewSelection, WorkBudget, WorkLimits, WorkspaceKey,
    WorktreeId, owner_map_key,
    ports::{ImmutableObjectPort, IndexFuture, PersistentMapPort},
};
use quanta_index_kernel::{AdapterDescriptor, AdapterRegistry, Kernel, KernelServices};
use quanta_index_semantic::native::{
    EmbeddingInput, EmbeddingInputPolicy, EmbeddingProvider, EmbeddingRecipe, EmbeddingVector,
    NativeSemantic, VectorNormalization,
};
use quanta_index_storage::{
    FileObjectStore, FileScratchSpace, ObjectIoObservation, ObjectIoSnapshot,
};

type TestResult<T = ()> = Result<T, Box<dyn Error>>;

const PRODUCER: ProducerId = ProducerId([41; 16]);
const KIND: IndexKind = IndexKind([43; 16]);
const SCHEMA: SchemaId = SchemaId([44; 16]);
const FORMAT: StorageFormatId = StorageFormatId([45; 16]);

struct CountingScratch {
    inner: FileScratchSpace,
    allocations: AtomicU64,
}

type NoopFixture = (
    Kernel<SqliteCatalog>,
    Arc<FileObjectStore>,
    Arc<CountingScratch>,
    Arc<AtomicU64>,
    Vec<InstanceSpec>,
    Arc<ObjectIoObservation>,
);

struct NoopCost {
    scratch_allocations: u64,
    object_io: ObjectIoSnapshot,
    request_wire_bytes: u64,
}

impl ScratchSpacePort for CountingScratch {
    fn allocate(
        &self,
        request: ScratchRequest,
        budget: &mut WorkBudget,
    ) -> Result<Box<dyn ScratchAllocation>, quanta_index_core::index::IndexError> {
        let allocation = self.inner.allocate(request, budget)?;
        let _previous = self.allocations.fetch_add(1, Ordering::SeqCst);
        Ok(allocation)
    }
}

struct FixedProvider {
    calls: Arc<AtomicU64>,
    recipe: EmbeddingRecipe,
}

impl EmbeddingProvider for FixedProvider {
    fn model_identity(&self) -> Digest {
        self.recipe.model_identity
    }

    fn dimension(&self) -> u32 {
        self.recipe.dimension
    }

    fn normalization(&self) -> VectorNormalization {
        self.recipe.normalization
    }

    fn output_policy(&self) -> Digest {
        self.recipe.output_policy
    }

    fn embed<'a>(
        &'a self,
        _input: EmbeddingInput<'a>,
    ) -> IndexFuture<'a, quanta_index_semantic::native::EmbeddingOutput> {
        let _previous = self.calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(async {
            Ok(quanta_index_semantic::native::EmbeddingOutput::unscoped(
                vec![EmbeddingVector {
                    id: b"whole-file".to_vec(),
                    values: vec![1.0, 0.0],
                }],
            ))
        })
    }
}

fn workspace() -> WorkspaceKey {
    WorkspaceKey {
        tenant: TenantId([1; 16]),
        collection: CollectionId([2; 16]),
        worktree: WorktreeId([3; 16]),
        slot: SlotId([4; 16]),
    }
}

fn operation(sequence: u64) -> OperationKey {
    OperationKey {
        workspace: workspace(),
        epoch: 1,
        producer: PRODUCER,
        stream: StreamId([5; 16]),
        stream_epoch: 1,
        sequence,
    }
}

fn owner() -> OwnerKey {
    OwnerKey {
        producer: PRODUCER,
        stable_id: StableOwnerId([6; 16]),
    }
}

fn recipe() -> EmbeddingRecipe {
    EmbeddingRecipe {
        input_policy: EmbeddingInputPolicy::SourceUtf8,
        model_identity: Digest([21; 32]),
        dimension: 2,
        normalization: VectorNormalization::L2Unit,
        chunk_policy: None,
        output_policy: Digest([22; 32]),
        max_vectors: 1,
        max_source_bytes: 1024,
    }
}

fn spec(index: u8, recipe: &EmbeddingRecipe) -> InstanceSpec {
    InstanceSpec {
        instance: IndexInstanceId([index; 16]),
        kind: KIND,
        group: PublicationGroupId([index; 16]),
        activation: GroupActivation::AsyncCoverage,
        input_domain: InputDomain::SourceLocal,
        schema: SCHEMA,
        policy: recipe.fingerprint(),
        format: FORMAT,
    }
}

fn budget() -> WorkBudget {
    WorkBudget::with_request(
        WorkLimits {
            reads: 1_000_000,
            writes: 1_000_000,
            bytes: 128 * 1024 * 1024,
            mutations: 1_000_000,
        },
        RequestBudgetV1::for_duration(Duration::from_secs(30)),
    )
}

fn published(outcome: SubmitOutcome) -> TestResult<ViewRef> {
    match outcome {
        SubmitOutcome::Terminal(OperationStatus::Published(view)) => Ok(view),
        other @ (SubmitOutcome::Accepted(_)
        | SubmitOutcome::Terminal(_)
        | SubmitOutcome::Unknown(_)) => Err(format!("publication failed: {other:?}").into()),
    }
}

fn source(engine: &Kernel<SqliteCatalog>, view: ViewRef) -> TestResult<SourceCut> {
    Ok(engine
        .pin_view(ViewSelection::Exact(view), &budget())?
        .view()
        .source)
}

fn coverage(
    engine: &Kernel<SqliteCatalog>,
    objects: &FileObjectStore,
    view: ViewRef,
    instance: IndexInstanceId,
) -> TestResult<CoverageBinding> {
    let ticket = engine.pin_view(ViewSelection::Exact(view), &budget())?;
    let mut key = b"member\0".to_vec();
    key.extend_from_slice(&instance.0);
    let reference = objects
        .lookup_object(ticket.view().coverage, &key, &mut budget())?
        .ok_or("coverage absent")?;
    Ok(serde_json::from_slice(
        &objects.get(reference, &mut budget())?.payload,
    )?)
}

fn fixture(root: &Path, async_instances: u8) -> TestResult<NoopFixture> {
    let scratch = Arc::new(CountingScratch {
        inner: FileScratchSpace::open_standalone(root.join("scratch"), 16 * 1024 * 1024 * 1024)?,
        allocations: AtomicU64::new(0),
    });
    let object_io = Arc::new(ObjectIoObservation::default());
    let objects = Arc::new(
        FileObjectStore::open_with_scratch(root.join("native-objects"), scratch.clone())?
            .with_io_observation(object_io.clone()),
    );
    let catalog = Arc::new(SqliteCatalog::open(root, Duration::from_millis(100))?);
    let calls = Arc::new(AtomicU64::new(0));
    let recipe = recipe();
    let mut registry = AdapterRegistry::new();
    registry.register_owner(
        AdapterDescriptor {
            kind: KIND,
            schema: SCHEMA,
            format: FORMAT,
        },
        NativeSemantic::new(
            FixedProvider {
                calls: calls.clone(),
                recipe: recipe.clone(),
            },
            recipe.clone(),
        )?,
    )?;
    let specs = (1..=async_instances)
        .map(|index| spec(index, &recipe))
        .collect();
    Ok((
        Kernel::new(
            KernelServices::new(
                objects.clone(),
                objects.clone(),
                catalog,
                scratch.clone(),
                Duration::from_secs(30),
            )?,
            registry,
        ),
        objects,
        scratch,
        calls,
        specs,
        object_io,
    ))
}

fn configure(engine: &Kernel<SqliteCatalog>, specs: &[InstanceSpec]) -> TestResult<ViewRef> {
    let genesis = engine.create_workspace(workspace(), 1, &mut budget())?;
    let ingress = engine.begin_input(operation(1))?;
    let profile = engine.stage_profile(
        ingress,
        &IndexProfile {
            instances: specs.to_vec(),
        },
        &mut budget(),
    )?;
    published(block_on(engine.submit(
        UpdateRequest {
            operation: operation(1),
            expected_view: genesis,
            kind: UpdateKind::Reconfigure { profile },
        },
        &mut budget(),
    ))?)
}

fn frontier_only_request(
    engine: &Kernel<SqliteCatalog>,
    expected_view: ViewRef,
    predecessor: u64,
) -> TestResult<UpdateRequest> {
    let sequence = predecessor
        .checked_add(2)
        .ok_or("operation sequence overflow")?;
    let event_sequence = predecessor
        .checked_add(1)
        .ok_or("source event sequence overflow")?;
    let key = operation(sequence);
    let ingress = engine.begin_input(key)?;
    let owners = engine.stage_mutations(
        ingress,
        std::iter::empty::<Result<OwnerMutation, quanta_index_core::index::IndexError>>(),
        &mut budget(),
    )?;
    assert_eq!(owners.count, 0);
    let mut transition = SourceTransition {
        expected_source: source(engine, expected_view)?,
        owners,
        events: Vec::new(),
    };
    transition.events.push(SourceEvent {
        producer: PRODUCER,
        stream: StreamId([5; 16]),
        stream_epoch: 1,
        predecessor,
        sequence: event_sequence,
        payload: transition.owner_digest()?,
    });
    Ok(UpdateRequest {
        operation: key,
        expected_view,
        kind: UpdateKind::SourceAdvance(transition),
    })
}

fn submit_frontier_only(
    engine: &Kernel<SqliteCatalog>,
    expected_view: ViewRef,
    predecessor: u64,
) -> TestResult<(ViewRef, u64)> {
    let request = frontier_only_request(engine, expected_view, predecessor)?;
    // This is the published Contract wire, separate from the object store's
    // measured bytes. Its source-root digests have value-dependent JSON width.
    let request_wire_bytes = u64::try_from(serde_json::to_vec(&request)?.len())?;
    let view = published(block_on(engine.submit(request, &mut budget()))?)?;
    Ok((view, request_wire_bytes))
}

fn frontier_only_cost(async_instances: u8, prior_updates: u64) -> TestResult<NoopCost> {
    let directory = tempfile::tempdir()?;
    let (engine, objects, scratch, calls, specs, object_io) =
        fixture(directory.path(), async_instances)?;
    let mut configured = configure(&engine, &specs)?;
    for predecessor in 0..prior_updates {
        configured = submit_frontier_only(&engine, configured, predecessor)?.0;
    }

    assert!(
        engine
            .ready_work(workspace(), 1, 8, &mut budget())?
            .is_empty()
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    let before_source = source(&engine, configured)?;
    let before_coverage_root = engine
        .pin_view(ViewSelection::Exact(configured), &budget())?
        .view()
        .coverage;
    let before_coverage = specs
        .iter()
        .map(|spec| coverage(&engine, &objects, configured, spec.instance))
        .collect::<TestResult<Vec<_>>>()?;
    for binding in &before_coverage {
        assert_eq!(binding.status, CoverageStatus::Complete);
        assert_eq!(binding.covered, Some(binding.desired));
    }

    let object_io_before = object_io.snapshot()?;
    let allocations_before = scratch.allocations.load(Ordering::SeqCst);
    let (after, request_wire_bytes) = submit_frontier_only(&engine, configured, prior_updates)?;
    let measured_object_io = object_io.snapshot()?.checked_since(&object_io_before)?;
    let delta_allocations = scratch
        .allocations
        .load(Ordering::SeqCst)
        .checked_sub(allocations_before)
        .ok_or("scratch allocation counter regressed")?;

    let after_source = source(&engine, after)?;
    let after_coverage_root = engine
        .pin_view(ViewSelection::Exact(after), &budget())?
        .view()
        .coverage;
    assert_eq!(after_source.owners, before_source.owners);
    assert_eq!(after_source.paths, before_source.paths);
    assert_ne!(after_source.frontiers, before_source.frontiers);
    if async_instances > 0 {
        assert_ne!(
            after_coverage_root, before_coverage_root,
            "rebinding async coverage must publish a changed coverage root"
        );
    }
    assert!(
        engine
            .ready_work(workspace(), 1, 8, &mut budget())?
            .is_empty()
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    for (spec, before) in specs.iter().zip(&before_coverage) {
        let after = coverage(&engine, &objects, after, spec.instance)?;
        assert_eq!(after.status, CoverageStatus::Complete);
        assert_eq!(after.covered, Some(after.desired));
        assert_eq!(after.desired.root, after_source.owners);
        assert_ne!(after.desired, before.desired);
        assert_eq!(after.owners, before.owners);
        assert_eq!(after.searchable, before.searchable);
    }
    Ok(NoopCost {
        scratch_allocations: delta_allocations,
        object_io: measured_object_io,
        request_wire_bytes,
    })
}

#[test]
fn frontier_only_source_advance_has_no_per_async_instance_scratch() -> TestResult {
    let none = frontier_only_cost(0, 0)?.scratch_allocations;
    let one = frontier_only_cost(1, 0)?.scratch_allocations;
    let three = frontier_only_cost(3, 0)?.scratch_allocations;
    assert!(none > 0, "source/publication scratch must remain visible");
    assert!(
        one >= none,
        "async coverage cannot remove base scratch work"
    );
    assert_eq!(
        three, one,
        "extra unchanged async indexes allocated scratch"
    );
    Ok(())
}

#[test]
fn frontier_only_object_install_count_does_not_grow_with_event_history() -> TestResult {
    // The producer advances one existing stream without changing any owner.
    // Every case checks the old/new source roots and complete async coverage
    // inside frontier_only_cost. This assertion counts actual object-store
    // calls; Catalog/WAL, scratch and device writes are outside its scope.
    let baseline = frontier_only_cost(1, 0)?;
    assert!(baseline.object_io.write_calls > 0);
    assert!(baseline.object_io.written_bytes > 0);
    assert!(baseline.object_io.reserve_objects > 0);
    assert_eq!(
        baseline.object_io.reserve_objects,
        baseline.object_io.confirm_objects
    );
    for history in [8, 32] {
        let observed = frontier_only_cost(1, history)?;
        assert_eq!(
            observed.scratch_allocations, baseline.scratch_allocations,
            "frontier-only scratch allocations grew with event history {history}"
        );
        assert_eq!(
            observed.object_io.write_calls, baseline.object_io.write_calls,
            "frontier-only object writes grew with event history {history}"
        );
        assert_eq!(
            observed.object_io.reserve_objects, baseline.object_io.reserve_objects,
            "frontier-only object custody grew with event history {history}"
        );
        assert_eq!(
            observed.object_io.reserve_objects,
            observed.object_io.confirm_objects
        );
        assert_eq!(
            (
                observed.object_io.failed_writes,
                observed.object_io.failed_reserves,
                observed.object_io.failed_confirms,
            ),
            (0, 0, 0)
        );
    }
    Ok(())
}

fn nonempty_frontier_cost(owner_count: u8, prior_updates: u64) -> TestResult<NoopCost> {
    if owner_count == 0 {
        return Err("nonempty source cost requires an owner".into());
    }
    let directory = tempfile::tempdir()?;
    let (engine, objects, scratch, calls, specs, object_io) = fixture(directory.path(), 0)?;
    let configured = configure(&engine, &specs)?;
    let ingress = engine.begin_input(operation(2))?;
    let mut expected = Vec::new();
    let mut mutations = Vec::new();
    for id in 1..=owner_count {
        let path = format!("src/{id}.rs");
        let reference = engine.stage_source_text(
            ingress,
            &path,
            b"needle",
            SourceTextAdmission::Utf8 {
                policy: Digest([8; 32]),
            },
            &mut budget(),
        )?;
        let key = OwnerKey {
            producer: PRODUCER,
            stable_id: StableOwnerId([id; 16]),
        };
        expected.push((key, path, reference));
        mutations.push(Ok(OwnerMutation {
            owner: key,
            expected: None,
            next: Some(reference),
        }));
    }
    let owners = engine.stage_mutations(ingress, mutations.into_iter(), &mut budget())?;
    assert_eq!(owners.count, u64::from(owner_count));
    let mut transition = SourceTransition {
        expected_source: source(&engine, configured)?,
        owners,
        events: Vec::new(),
    };
    transition.events.push(SourceEvent {
        producer: PRODUCER,
        stream: StreamId([5; 16]),
        stream_epoch: 1,
        predecessor: 0,
        sequence: 1,
        payload: transition.owner_digest()?,
    });
    let mut current = published(block_on(engine.submit(
        UpdateRequest {
            operation: operation(2),
            expected_view: configured,
            kind: UpdateKind::SourceAdvance(transition),
        },
        &mut budget(),
    ))?)?;
    for predecessor in 1..=prior_updates {
        current = submit_frontier_only(&engine, current, predecessor)?.0;
    }
    let old_ticket = engine.pin_view(ViewSelection::Exact(current), &budget())?;
    let old_source = old_ticket.view().source;
    let io_before = object_io.snapshot()?;
    let scratch_before = scratch.allocations.load(Ordering::SeqCst);
    let measured_predecessor = prior_updates
        .checked_add(1)
        .ok_or("source event history overflow")?;
    let (next, request_wire_bytes) = submit_frontier_only(&engine, current, measured_predecessor)?;
    let object_io = object_io.snapshot()?.checked_since(&io_before)?;
    let scratch_allocations = scratch
        .allocations
        .load(Ordering::SeqCst)
        .checked_sub(scratch_before)
        .ok_or("scratch allocation counter regressed")?;
    let new_source = source(&engine, next)?;
    assert_eq!(new_source.owners, old_source.owners);
    assert_eq!(new_source.paths, old_source.paths);
    assert_ne!(new_source.frontiers, old_source.frontiers);
    for (key, path, reference) in expected {
        let map_key = owner_map_key(key);
        assert_eq!(
            objects.lookup_object(old_source.owners, &map_key, &mut budget())?,
            Some(reference)
        );
        assert_eq!(
            objects.lookup_object(new_source.owners, &map_key, &mut budget())?,
            Some(reference)
        );
        let record: SourceRecord =
            serde_json::from_slice(&objects.get(reference, &mut budget())?.payload)?;
        assert_eq!(record.path, path);
        assert_eq!(
            objects.get(record.bytes, &mut budget())?.payload.as_slice(),
            b"needle"
        );
    }
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    old_ticket.release()?;
    Ok(NoopCost {
        scratch_allocations,
        object_io,
        request_wire_bytes,
    })
}

#[test]
fn frontier_only_physical_object_work_is_bounded_across_source_size_and_history() -> TestResult {
    // The object-store observer excludes Catalog/WAL, scratch and device IO.
    // An empty owner delta must not install one additional object per prior
    // source owner or per already published stream event.
    let baseline = nonempty_frontier_cost(1, 0)?;
    assert!(baseline.object_io.write_calls > 0);
    let baseline_nonrequest_bytes = baseline
        .object_io
        .written_bytes
        .checked_sub(baseline.request_wire_bytes)
        .ok_or("measured writes omitted the immutable request wire")?;
    for (owners, history) in [(64, 0), (64, 8), (64, 32)] {
        let observed = nonempty_frontier_cost(owners, history)?;
        if history == 0 {
            // The canonical request JSON records the different pinned source
            // digests as decimal u8 arrays. Subtract only that independently
            // serialized request payload; all remaining installed wire bytes
            // must have the same shape for an unchanged-owner frontier edit.
            let nonrequest_bytes = observed
                .object_io
                .written_bytes
                .checked_sub(observed.request_wire_bytes)
                .ok_or("measured writes omitted the immutable request wire")?;
            assert_eq!(
                nonrequest_bytes, baseline_nonrequest_bytes,
                "frontier-only non-request object bytes grew with unchanged source owners"
            );
        }
        assert_eq!(
            observed.scratch_allocations, baseline.scratch_allocations,
            "frontier-only scratch cost grew with {owners} owners / {history} events"
        );
        assert_eq!(
            observed.object_io.write_calls, baseline.object_io.write_calls,
            "frontier-only object writes grew with {owners} owners / {history} events"
        );
        assert_eq!(
            observed.object_io.reserve_objects, baseline.object_io.reserve_objects,
            "frontier-only object custody grew with {owners} owners / {history} events"
        );
    }
    Ok(())
}

#[test]
fn frontier_only_source_advance_preserves_partial_owner_work() -> TestResult {
    let directory = tempfile::tempdir()?;
    let (engine, objects, _scratch, calls, specs, _object_io) = fixture(directory.path(), 1)?;
    let configured = configure(&engine, &specs)?;
    let ingress = engine.begin_input(operation(2))?;
    let record = engine.stage_source_text(
        ingress,
        "a.rs",
        b"alpha",
        SourceTextAdmission::Utf8 {
            policy: Digest([8; 32]),
        },
        &mut budget(),
    )?;
    let owners = engine.stage_mutations(
        ingress,
        std::iter::once(Ok(OwnerMutation {
            owner: owner(),
            expected: None,
            next: Some(record),
        })),
        &mut budget(),
    )?;
    let mut first = SourceTransition {
        expected_source: source(&engine, configured)?,
        owners,
        events: Vec::new(),
    };
    first.events.push(SourceEvent {
        producer: PRODUCER,
        stream: StreamId([5; 16]),
        stream_epoch: 1,
        predecessor: 0,
        sequence: 1,
        payload: first.owner_digest()?,
    });
    let partial_view = published(block_on(engine.submit(
        UpdateRequest {
            operation: operation(2),
            expected_view: configured,
            kind: UpdateKind::SourceAdvance(first),
        },
        &mut budget(),
    ))?)?;
    let instance = specs.first().ok_or("one async instance required")?.instance;
    let prior = coverage(&engine, &objects, partial_view, instance)?;
    let CoverageStatus::Partial { missing } = prior.status else {
        return Err("initial owner must remain pending".into());
    };
    assert!(missing.0.is_some());
    assert_eq!(prior.covered, None);
    let ready_before = engine.ready_work(workspace(), 1, 8, &mut budget())?;
    let [_intent] = ready_before.as_slice() else {
        return Err("one owner work item must remain ready".into());
    };
    assert_eq!(calls.load(Ordering::SeqCst), 0);

    let prior_source = source(&engine, partial_view)?;
    let ingress = engine.begin_input(operation(3))?;
    let empty = engine.stage_mutations(
        ingress,
        std::iter::empty::<Result<OwnerMutation, quanta_index_core::index::IndexError>>(),
        &mut budget(),
    )?;
    assert_eq!(empty.count, 0);
    let mut frontier_only = SourceTransition {
        expected_source: prior_source,
        owners: empty,
        events: Vec::new(),
    };
    frontier_only.events.push(SourceEvent {
        producer: PRODUCER,
        stream: StreamId([5; 16]),
        stream_epoch: 1,
        predecessor: 1,
        sequence: 2,
        payload: frontier_only.owner_digest()?,
    });
    let after_view = published(block_on(engine.submit(
        UpdateRequest {
            operation: operation(3),
            expected_view: partial_view,
            kind: UpdateKind::SourceAdvance(frontier_only),
        },
        &mut budget(),
    ))?)?;
    let next_source = source(&engine, after_view)?;
    assert_eq!(next_source.owners, prior_source.owners);
    assert_ne!(next_source.frontiers, prior_source.frontiers);
    let after = coverage(&engine, &objects, after_view, instance)?;
    assert_eq!(after.status, CoverageStatus::Partial { missing });
    assert_eq!(after.covered, None);
    assert_eq!(after.owners, prior.owners);
    assert_eq!(after.searchable, prior.searchable);
    assert_ne!(after.desired, prior.desired);
    assert_eq!(
        engine.ready_work(workspace(), 1, 8, &mut budget())?,
        ready_before,
        "frontier-only update must not enqueue or retire owner work"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    Ok(())
}

#[test]
fn forged_frontier_only_manifest_never_publishes() -> TestResult {
    // SourceAdvance validates the manifest before any adapter runs. These
    // cases protect end-to-end nonpublication, not the async cursor in isolation.
    for forge_count in [true, false] {
        let directory = tempfile::tempdir()?;
        let (engine, objects, _scratch, calls, specs, _object_io) = fixture(directory.path(), 1)?;
        let configured = configure(&engine, &specs)?;
        let instance = specs.first().ok_or("one async instance required")?.instance;
        let original_coverage = coverage(&engine, &objects, configured, instance)?;
        let original_source = source(&engine, configured)?;
        assert!(
            engine
                .ready_work(workspace(), 1, 8, &mut budget())?
                .is_empty()
        );

        let key = operation(2);
        let ingress = engine.begin_input(key)?;
        let mut owners = engine.stage_mutations(
            ingress,
            std::iter::empty::<Result<OwnerMutation, quanta_index_core::index::IndexError>>(),
            &mut budget(),
        )?;
        assert_eq!(owners.count, 0);
        if forge_count {
            owners.count = 1;
        } else {
            owners.digest = Digest([93; 32]);
        }
        let mut transition = SourceTransition {
            expected_source: original_source,
            owners,
            events: Vec::new(),
        };
        transition.events.push(SourceEvent {
            producer: PRODUCER,
            stream: StreamId([5; 16]),
            stream_epoch: 1,
            predecessor: 0,
            sequence: 1,
            payload: transition.owner_digest()?,
        });
        let outcome = block_on(engine.submit(
            UpdateRequest {
                operation: key,
                expected_view: configured,
                kind: UpdateKind::SourceAdvance(transition),
            },
            &mut budget(),
        ))?;
        assert_eq!(
            outcome,
            SubmitOutcome::Unknown(key),
            "corrupt manifest must leave no published view"
        );
        assert!(matches!(
            engine.status(
                key,
                quanta_index_core::index::ports::OperationInspectAccess::Request(&budget())
            )?,
            OperationStatus::Pending(_)
        ));
        let latest = engine.pin_view(
            ViewSelection::Latest {
                workspace: workspace(),
                epoch: 1,
            },
            &budget(),
        )?;
        assert_eq!(latest.view().identity, configured);
        assert_eq!(latest.view().source, original_source);
        assert_eq!(
            coverage(&engine, &objects, configured, instance)?,
            original_coverage
        );
        assert!(
            engine
                .ready_work(workspace(), 1, 8, &mut budget())?
                .is_empty()
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }
    Ok(())
}
