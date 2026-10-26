//! Real owner materialization goldens for semantic metadata-only changes.
#![expect(
    clippy::disallowed_methods,
    reason = "test-only synchronous driver executes Kernel futures"
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
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use futures::executor::block_on;
use quanta_index_catalog::SqliteCatalog;
use quanta_index_contract::{EmbeddingNormalization, native_protocol::NativeSemanticTraversal};
use quanta_index_core::index::{
    AttemptRef, CollectionId, CoverageBinding, Digest, GroupActivation, IndexError,
    IndexInstanceId, IndexKind, IndexProfile, InputDomain, InstanceSpec, MapBatchRequest,
    MapBatchWrite, MapEdit, MapPage, MapRoot, MapValue, MapWrite, ObjectBatchInput, ObjectRef,
    OperationKey, OperationStatus, OwnerCoverage, OwnerKey, OwnerMemberBinding, OwnerMutation,
    ProducerId, PublicationGroupId, RegisteredObject, RegisteredObjectBatch, SchemaId, SlotId,
    SourceCut, SourceEvent, SourceRecord, SourceTextAdmission, SourceTransition, StableOwnerId,
    StagedMapRoots, StagedObjectReferences, StorageFormatId, StoredObject, StreamId, SubmitOutcome,
    TenantId, UpdateKind, UpdateRequest, ViewRef, ViewSelection, WorkBudget, WorkLimits,
    WorkspaceKey, WorktreeId, owner_map_key,
    ports::{
        CatalogReadPort, ImmutableObjectPort, IndexFuture, MapEditCursor, ObjectMapBatchPort,
        ObjectWriteSink, PersistentMapPort, PreparedObjectMapBatch,
    },
};
use quanta_index_core::{CoreError, RequestBudgetV1, TextEmbeddingProvider};
use quanta_index_kernel::{AdapterDescriptor, AdapterRegistry, Kernel, KernelServices};
use quanta_index_semantic::native::{
    EmbeddingInput, EmbeddingInputPolicy, EmbeddingProvider, EmbeddingRecipe, EmbeddingVector,
    NativeSemantic, ReadContext, SearchResult, TextEmbeddingBridge, TicketSearchQuery,
    VectorNormalization, search_exact_acquired,
};
use quanta_index_storage::{FileObjectStore, ObjectIoObservation, ObjectIoSnapshot};

type TestResult<T = ()> = Result<T, Box<dyn Error>>;
const PRODUCER: ProducerId = ProducerId([41; 16]);
const INSTANCE: IndexInstanceId = IndexInstanceId([42; 16]);
const KIND: IndexKind = IndexKind([43; 16]);
const SCHEMA: SchemaId = SchemaId([44; 16]);
const FORMAT: StorageFormatId = StorageFormatId([45; 16]);

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

fn spec(recipe: &EmbeddingRecipe) -> InstanceSpec {
    InstanceSpec {
        instance: INSTANCE,
        kind: KIND,
        group: PublicationGroupId([7; 16]),
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

fn kernel<P: EmbeddingProvider + 'static>(
    root: &Path,
    provider: P,
    recipe: EmbeddingRecipe,
) -> TestResult<(
    Kernel<SqliteCatalog>,
    Arc<FileObjectStore>,
    Arc<SqliteCatalog>,
)> {
    let scratch = Arc::new(quanta_index_storage::FileScratchSpace::open_standalone(
        root.join("scratch"),
        16 * 1024 * 1024 * 1024,
    )?);
    let objects = Arc::new(FileObjectStore::open_with_scratch(
        root.join("native-objects"),
        scratch.clone(),
    )?);
    let catalog = Arc::new(SqliteCatalog::open(root, Duration::from_millis(100))?);
    let mut registry = AdapterRegistry::new();
    registry.register_owner(
        AdapterDescriptor {
            kind: KIND,
            schema: SCHEMA,
            format: FORMAT,
        },
        NativeSemantic::new(provider, recipe)?,
    )?;
    Ok((
        Kernel::new(
            KernelServices::new(
                objects.clone(),
                objects.clone(),
                catalog.clone(),
                scratch,
                Duration::from_secs(30),
            )?,
            registry,
        ),
        objects,
        catalog,
    ))
}

/// The old linked backend has the same object/map implementation and delta
/// join, but does not offer the new owner-map preparation capability.
struct OriginalOwnerMapBackend {
    store: Arc<FileObjectStore>,
}

impl ImmutableObjectPort for OriginalOwnerMapBackend {
    fn identify(
        &self,
        payload: &[u8],
        children: &[ObjectRef],
        budget: &mut WorkBudget,
    ) -> Result<RegisteredObject, IndexError> {
        self.store.identify(payload, children, budget)
    }

    fn put(
        &self,
        payload: &[u8],
        children: &[ObjectRef],
        budget: &mut WorkBudget,
    ) -> Result<RegisteredObject, IndexError> {
        self.store.put(payload, children, budget)
    }

    fn install(
        &self,
        payload: &[u8],
        children: &[ObjectRef],
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<RegisteredObject, IndexError> {
        self.store.install(payload, children, sink, budget)
    }

    fn install_batch(
        &self,
        inputs: &[ObjectBatchInput],
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<RegisteredObjectBatch, IndexError> {
        self.store.install_batch(inputs, sink, budget)
    }

    fn get(
        &self,
        reference: ObjectRef,
        budget: &mut WorkBudget,
    ) -> Result<StoredObject, IndexError> {
        self.store.get(reference, budget)
    }

    fn read_retained(
        &self,
        object: &StoredObject,
        budget: &mut WorkBudget,
    ) -> Result<StoredObject, IndexError> {
        self.store.read_retained(object, budget)
    }
}

impl PersistentMapPort for OriginalOwnerMapBackend {
    fn scoped_reads<'a>(
        &'a self,
        budget: &WorkBudget,
    ) -> Result<Option<Box<dyn PersistentMapPort + 'a>>, IndexError> {
        self.store.scoped_reads(budget)
    }

    fn lookup(
        &self,
        root: MapRoot,
        key: &[u8],
        budget: &mut WorkBudget,
    ) -> Result<Option<MapValue>, IndexError> {
        self.store.lookup(root, key, budget)
    }

    fn lookup_batch(
        &self,
        root: MapRoot,
        keys: &[&[u8]],
        values: &mut [Option<MapValue>],
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        self.store.lookup_batch(root, keys, values, budget)
    }

    fn scan(
        &self,
        root: MapRoot,
        after: Option<&[u8]>,
        limit: u32,
        budget: &mut WorkBudget,
    ) -> Result<MapPage, IndexError> {
        self.store.scan(root, after, limit, budget)
    }

    fn visit_diff(
        &self,
        before: MapRoot,
        after: MapRoot,
        range: quanta_index_core::index::ports::MapDiffRange<'_>,
        visitor: &mut dyn quanta_index_core::index::ports::MapDiffVisitor,
        budget: &mut WorkBudget,
    ) -> Result<bool, IndexError> {
        self.store.visit_diff(before, after, range, visitor, budget)
    }

    fn apply(
        &self,
        base: MapRoot,
        edits: &[MapEdit],
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<MapWrite, IndexError> {
        self.store.apply(base, edits, sink, budget)
    }

    fn apply_batch(
        &self,
        requests: &[MapBatchRequest],
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<MapBatchWrite, IndexError> {
        self.store.apply_batch(requests, sink, budget)
    }

    fn apply_stream(
        &self,
        bases: &[MapRoot],
        edits: &mut dyn MapEditCursor,
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<StagedMapRoots, IndexError> {
        self.store.apply_stream(bases, edits, sink, budget)
    }
}

impl ObjectMapBatchPort for OriginalOwnerMapBackend {
    fn install_objects_and_maps(
        &self,
        objects: &[ObjectBatchInput],
        bases: &[MapRoot],
        edits: &[&MapEdit],
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<(StagedObjectReferences, StagedMapRoots), IndexError> {
        self.store
            .install_objects_and_maps(objects, bases, edits, sink, budget)
    }

    fn prepare_one_map(
        self: Arc<Self>,
        base: MapRoot,
        edit: &MapEdit,
        attempt: AttemptRef,
        budget: &mut WorkBudget,
    ) -> Result<Box<dyn PreparedObjectMapBatch>, IndexError> {
        Arc::clone(&self.store).prepare_one_map(base, edit, attempt, budget)
    }
}

struct AnnJoinWitness {
    view: ViewRef,
    coverage: CoverageBinding,
    owner_coverage: OwnerCoverage,
    member: OwnerMemberBinding,
    descriptor: ObjectRef,
    payload: Vec<u8>,
    children: Vec<ObjectRef>,
    installs: ObjectIoSnapshot,
    cosine: f64,
}

fn ann_delta_join_witness(prepare_owner_maps: bool) -> TestResult<AnnJoinWitness> {
    let directory = tempfile::tempdir()?;
    let scratch = Arc::new(quanta_index_storage::FileScratchSpace::open_standalone(
        directory.path().join("scratch"),
        16 * 1024 * 1024 * 1024,
    )?);
    let observer = Arc::new(ObjectIoObservation::default());
    let objects = Arc::new(
        FileObjectStore::open_with_scratch(
            directory.path().join("native-objects"),
            scratch.clone(),
        )?
        .with_io_observation(Arc::clone(&observer)),
    );
    let catalog = Arc::new(SqliteCatalog::open(
        directory.path(),
        Duration::from_millis(100),
    )?);
    let calls = Arc::new(AtomicUsize::new(0));
    let provider = Arc::new(TextEmbeddingBridge::new(FixedTextProvider {
        calls: Arc::clone(&calls),
    })?);
    let recipe = provider.recipe(1024)?;
    let mut registry = AdapterRegistry::new();
    registry.register_owner(
        AdapterDescriptor {
            kind: KIND,
            schema: SCHEMA,
            format: FORMAT,
        },
        NativeSemantic::new(provider, recipe.clone())?.with_ann(),
    )?;
    let services = KernelServices::new(
        objects.clone(),
        objects.clone(),
        catalog.clone(),
        scratch,
        Duration::from_secs(30),
    )?;
    let services = if prepare_owner_maps {
        services.with_object_map_backend(objects.clone())
    } else {
        services.with_object_map_backend(Arc::new(OriginalOwnerMapBackend {
            store: objects.clone(),
        }))
    };
    let kernel = Kernel::new(services, registry);
    let configured = configure(&kernel, &recipe)
        .map_err(|error| format!("owner scope={prepare_owner_maps}: configure: {error}"))?;
    let (pending, _record) = advance(&kernel, configured, 2, 1, "a.rs", b"alpha", None)
        .map_err(|error| format!("owner scope={prepare_owner_maps}: advance: {error}"))?;
    let before = observer.snapshot()?;
    let view = materialize(&kernel, pending, 3)
        .map_err(|error| format!("owner scope={prepare_owner_maps}: materialize: {error}"))?;
    let installs = observer.snapshot()?.checked_since(&before)?;
    let ticket = kernel.pin_view(ViewSelection::Exact(view), &budget())?;
    let mut key = b"member\0".to_vec();
    key.extend_from_slice(&INSTANCE.0);
    let coverage_ref = objects
        .lookup_object(ticket.view().coverage, &key, &mut budget())?
        .ok_or("ANN coverage absent")?;
    let coverage: CoverageBinding =
        serde_json::from_slice(&objects.get(coverage_ref, &mut budget())?.payload)?;
    let owner_ref = objects
        .lookup_object(coverage.owners, &owner_map_key(owner()), &mut budget())?
        .ok_or("ANN owner coverage absent")?;
    let owner_coverage: OwnerCoverage =
        serde_json::from_slice(&objects.get(owner_ref, &mut budget())?.payload)?;
    let member_ref = owner_coverage.searchable.ok_or("ANN owner member absent")?;
    let member: OwnerMemberBinding =
        serde_json::from_slice(&objects.get(member_ref, &mut budget())?.payload)?;
    let descriptor = coverage.searchable.ok_or("ANN descriptor absent")?;
    let stored = objects.get(descriptor, &mut budget())?;
    let cosine = cosine_at(&catalog, &objects, &recipe, view)?;
    let replay_before = observer.snapshot()?;
    assert_eq!(
        block_on(kernel.resume(operation(3), &mut budget()))?,
        SubmitOutcome::Terminal(OperationStatus::Published(view))
    );
    let replay = observer.snapshot()?.checked_since(&replay_before)?;
    assert_eq!(replay.reserve_calls, 0);
    assert_eq!(replay.confirm_calls, 0);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    ticket.release()?;
    Ok(AnnJoinWitness {
        view,
        coverage,
        owner_coverage,
        member,
        descriptor,
        payload: stored.payload.clone(),
        children: stored.registration.children.clone(),
        installs,
        cosine,
    })
}

#[test]
fn ann_nonseal_delta_joins_original_descriptor_coverage_install() -> TestResult {
    let original = ann_delta_join_witness(false)?;
    let joined = ann_delta_join_witness(true)?;
    assert_eq!(joined.view, original.view);
    assert_eq!(joined.coverage, original.coverage);
    assert_eq!(joined.owner_coverage, original.owner_coverage);
    assert_eq!(joined.member, original.member);
    assert_eq!(joined.descriptor, original.descriptor);
    assert_eq!(joined.payload, original.payload);
    assert_eq!(joined.children, original.children);
    assert_eq!(
        joined.cosine.total_cmp(&original.cosine),
        std::cmp::Ordering::Equal
    );
    // Both paths join delta and coverage. The new owner-map scope also joins
    // metadata, while the old linked backend installed metadata separately.
    assert_eq!(original.installs.reserve_calls, 4);
    assert_eq!(original.installs.confirm_calls, 4);
    assert_eq!(joined.installs.reserve_calls, 3);
    assert_eq!(joined.installs.confirm_calls, 3);
    assert_eq!(
        original.installs.reserve_calls,
        joined
            .installs
            .reserve_calls
            .checked_add(1)
            .ok_or("reserve count overflow")?
    );
    assert_eq!(
        original.installs.confirm_calls,
        joined
            .installs
            .confirm_calls
            .checked_add(1)
            .ok_or("confirm count overflow")?
    );
    assert_eq!(
        original.installs.reserve_objects,
        joined.installs.reserve_objects
    );
    assert_eq!(
        original.installs.confirm_objects,
        joined.installs.confirm_objects
    );
    assert_eq!(
        original.installs.file_sync_calls,
        joined.installs.file_sync_calls
    );
    #[cfg(target_os = "macos")]
    assert_eq!(
        original.installs.barrier_calls,
        joined
            .installs
            .barrier_calls
            .checked_add(1)
            .ok_or("barrier count overflow")?
    );
    Ok(())
}

fn source(kernel: &Kernel<SqliteCatalog>, view: ViewRef) -> TestResult<SourceCut> {
    Ok(kernel
        .pin_view(ViewSelection::Exact(view), &budget())?
        .view()
        .source)
}

fn configure(kernel: &Kernel<SqliteCatalog>, recipe: &EmbeddingRecipe) -> TestResult<ViewRef> {
    let genesis = kernel.create_workspace(workspace(), 1, &mut budget())?;
    let ingress = kernel.begin_input(operation(1))?;
    let profile = kernel.stage_profile(
        ingress,
        &IndexProfile {
            instances: vec![spec(recipe)],
        },
        &mut budget(),
    )?;
    published(block_on(kernel.submit(
        UpdateRequest {
            operation: operation(1),
            expected_view: genesis,
            kind: UpdateKind::Reconfigure { profile },
        },
        &mut budget(),
    ))?)
}

fn advance(
    kernel: &Kernel<SqliteCatalog>,
    view: ViewRef,
    operation_sequence: u64,
    event_sequence: u64,
    path: &str,
    bytes: &[u8],
    previous: Option<ObjectRef>,
) -> TestResult<(ViewRef, ObjectRef)> {
    let key = operation(operation_sequence);
    let ingress = kernel.begin_input(key)?;
    let record = kernel.stage_source_text(
        ingress,
        path,
        bytes,
        SourceTextAdmission::Utf8 {
            policy: Digest([8; 32]),
        },
        &mut budget(),
    )?;
    let owners = kernel.stage_mutations(
        ingress,
        std::iter::once(Ok(OwnerMutation {
            owner: owner(),
            expected: previous,
            next: Some(record),
        })),
        &mut budget(),
    )?;
    let mut transition = SourceTransition {
        expected_source: source(kernel, view)?,
        owners,
        events: Vec::new(),
    };
    transition.events.push(SourceEvent {
        producer: PRODUCER,
        stream: StreamId([5; 16]),
        stream_epoch: 1,
        predecessor: event_sequence
            .checked_sub(1)
            .ok_or("source event sequence must start at one")?,
        sequence: event_sequence,
        payload: transition.owner_digest()?,
    });
    let next = published(block_on(kernel.submit(
        UpdateRequest {
            operation: key,
            expected_view: view,
            kind: UpdateKind::SourceAdvance(transition),
        },
        &mut budget(),
    ))?)?;
    Ok((next, record))
}

fn materialize(
    kernel: &Kernel<SqliteCatalog>,
    view: ViewRef,
    sequence: u64,
) -> TestResult<ViewRef> {
    let ready = kernel.ready_work(workspace(), 1, 8, &mut budget())?;
    let [ready] = ready.as_slice() else {
        return Err("expected one ready owner".into());
    };
    let claim = kernel.claim_work(ready, Duration::from_secs(30), &budget())?;
    published(block_on(kernel.submit(
        UpdateRequest {
            operation: operation(sequence),
            expected_view: view,
            kind: UpdateKind::Materialize { claim },
        },
        &mut budget(),
    ))?)
}

fn member(
    kernel: &Kernel<SqliteCatalog>,
    objects: &FileObjectStore,
    view: ViewRef,
) -> TestResult<OwnerMemberBinding> {
    let ticket = kernel.pin_view(ViewSelection::Exact(view), &budget())?;
    let mut key = b"member\0".to_vec();
    key.extend_from_slice(&INSTANCE.0);
    let coverage_ref = objects
        .lookup_object(ticket.view().coverage, &key, &mut budget())?
        .ok_or("coverage absent")?;
    let coverage: CoverageBinding =
        serde_json::from_slice(&objects.get(coverage_ref, &mut budget())?.payload)?;
    let owner_ref = objects
        .lookup_object(coverage.owners, &owner_map_key(owner()), &mut budget())?
        .ok_or("owner coverage absent")?;
    let owner_coverage: OwnerCoverage =
        serde_json::from_slice(&objects.get(owner_ref, &mut budget())?.payload)?;
    let member_ref = owner_coverage.searchable.ok_or("owner member absent")?;
    Ok(serde_json::from_slice(
        &objects.get(member_ref, &mut budget())?.payload,
    )?)
}

fn result_at(
    catalog: &SqliteCatalog,
    objects: &FileObjectStore,
    recipe: &EmbeddingRecipe,
    view: ViewRef,
) -> TestResult<SearchResult> {
    let mut work = budget();
    let mut read = ReadContext {
        objects,
        maps: objects,
        budget: &mut work,
    };
    let session = catalog.open_read_session()?;
    Ok(search_exact_acquired(
        catalog,
        session.as_ref(),
        Duration::from_secs(30),
        TicketSearchQuery {
            selection: ViewSelection::Exact(view),
            instance: INSTANCE,
            format: FORMAT,
            recipe,
            vector: &[1.0, 0.0],
            top_k: 1,
            page_size: 8,
            max_owners: 8,
        },
        &mut read,
    )?)
}

fn cosine_at(
    catalog: &SqliteCatalog,
    objects: &FileObjectStore,
    recipe: &EmbeddingRecipe,
    view: ViewRef,
) -> TestResult<f64> {
    let result = result_at(catalog, objects, recipe, view)?;
    assert!(result.exhaustive);
    let [hit] = result.hits.as_slice() else {
        return Err("expected one semantic hit".into());
    };
    Ok(hit.cosine)
}

struct FixedTextProvider {
    calls: Arc<AtomicUsize>,
}

impl TextEmbeddingProvider for FixedTextProvider {
    fn embed_batch(
        &self,
        texts: &[&str],
    ) -> Result<quanta_index_core::TextEmbeddingBatch, quanta_index_core::CoreError> {
        Ok(texts.iter().map(|_| vec![1.0, 0.0]).collect())
    }
    fn embed_batch_within(
        &self,
        texts: &[&str],
        budget: &RequestBudgetV1,
    ) -> Result<quanta_index_core::TextEmbeddingBatch, quanta_index_core::CoreError> {
        budget.checkpoint("semantic:embed")?;
        let _previous = self.calls.fetch_add(1, Ordering::SeqCst);
        let memory = quanta_index_core::TextEmbeddingBatch::reserve_output(
            texts.len(),
            self.dimension(),
            budget,
        )?;
        let vectors = self.embed_batch(texts)?.into_unscoped()?;
        Ok(quanta_index_core::TextEmbeddingBatch::from_parts(
            vectors, memory,
        ))
    }
    fn model_id(&self) -> &'static str {
        "fixed-rebind-model"
    }
    fn model_revision(&self) -> &'static str {
        "revision-1"
    }
    fn dimension(&self) -> usize {
        2
    }
    fn normalization(&self) -> EmbeddingNormalization {
        EmbeddingNormalization::L2Unit
    }
    fn supports_deterministic_payload_reuse(&self) -> bool {
        true
    }
}

struct PathSensitiveProvider {
    calls: Arc<AtomicUsize>,
    recipe: EmbeddingRecipe,
}

struct StatefulTextProvider {
    calls: Arc<AtomicUsize>,
}

impl TextEmbeddingProvider for StatefulTextProvider {
    fn embed_batch(
        &self,
        texts: &[&str],
    ) -> Result<quanta_index_core::TextEmbeddingBatch, CoreError> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        let vector = if call == 0 {
            vec![1.0, 0.0]
        } else {
            vec![0.0, 1.0]
        };
        Ok(texts.iter().map(|_| vector.clone()).collect())
    }

    fn embed_batch_within(
        &self,
        texts: &[&str],
        budget: &RequestBudgetV1,
    ) -> Result<quanta_index_core::TextEmbeddingBatch, CoreError> {
        budget.checkpoint("semantic:embed")?;
        let memory = quanta_index_core::TextEmbeddingBatch::reserve_output(
            texts.len(),
            self.dimension(),
            budget,
        )?;
        let vectors = self.embed_batch(texts)?.into_unscoped()?;
        Ok(quanta_index_core::TextEmbeddingBatch::from_parts(
            vectors, memory,
        ))
    }

    fn model_id(&self) -> &'static str {
        "stateful-rebind-model"
    }
    fn model_revision(&self) -> &'static str {
        "revision-1"
    }
    fn dimension(&self) -> usize {
        2
    }
    fn normalization(&self) -> EmbeddingNormalization {
        EmbeddingNormalization::L2Unit
    }
}

impl EmbeddingProvider for PathSensitiveProvider {
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
        input: EmbeddingInput<'a>,
    ) -> IndexFuture<'a, quanta_index_semantic::native::EmbeddingOutput> {
        let _previous = self.calls.fetch_add(1, Ordering::SeqCst);
        let result = match input.source.map(|source| source.path.as_str()) {
            Some("a.rs") => Ok(vec![1.0, 0.0]),
            Some("b.rs") => Ok(vec![0.0, 1.0]),
            _ => Err(quanta_index_core::index::IndexError::InvalidInput(
                "unexpected source path".into(),
            )),
        };
        Box::pin(async move {
            result.map(|values| {
                quanta_index_semantic::native::EmbeddingOutput::unscoped(vec![EmbeddingVector {
                    id: b"whole-file".to_vec(),
                    values,
                }])
            })
        })
    }
}

fn run_rename<P: EmbeddingProvider + 'static>(
    root: &Path,
    provider: P,
    recipe: &EmbeddingRecipe,
    calls: &AtomicUsize,
    expected_calls: usize,
    expected_new_cosine: f64,
) -> TestResult {
    let (kernel, objects, catalog) = kernel(root, provider, recipe.clone())?;
    let configured = configure(&kernel, recipe)?;
    let (first, a_record) = advance(&kernel, configured, 2, 1, "a.rs", b"alpha", None)?;
    let first = materialize(&kernel, first, 3)?;
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        member(&kernel, &objects, first)?.input.source,
        Some(a_record)
    );
    let old_pin = kernel.pin_view(ViewSelection::Exact(first), &budget())?;
    let (renamed, b_record) = advance(&kernel, first, 4, 2, "b.rs", b"alpha", Some(a_record))?;
    assert_ne!(a_record, b_record);
    let a: SourceRecord = serde_json::from_slice(&objects.get(a_record, &mut budget())?.payload)?;
    let b: SourceRecord = serde_json::from_slice(&objects.get(b_record, &mut budget())?.payload)?;
    assert_eq!(a.bytes, b.bytes);
    assert_eq!(old_pin.view().identity, first);
    let pending = result_at(&catalog, &objects, recipe, renamed)?;
    assert!(
        pending.hits.is_empty(),
        "stale prior vectors must be hidden"
    );
    assert_eq!(pending.traversal, NativeSemanticTraversal::ExactExhausted);
    assert!(!pending.exhaustive);
    assert_eq!(
        cosine_at(&catalog, &objects, recipe, first)?.total_cmp(&1.0),
        std::cmp::Ordering::Equal
    );
    let renamed = materialize(&kernel, renamed, 5)?;
    assert_eq!(
        member(&kernel, &objects, renamed)?.input.source,
        Some(b_record)
    );
    assert_eq!(
        member(&kernel, &objects, first)?.input.source,
        Some(a_record)
    );
    assert_eq!(
        cosine_at(&catalog, &objects, recipe, renamed)?.total_cmp(&expected_new_cosine),
        std::cmp::Ordering::Equal
    );
    assert_eq!(
        cosine_at(&catalog, &objects, recipe, first)?.total_cmp(&1.0),
        std::cmp::Ordering::Equal
    );
    assert_eq!(calls.load(Ordering::SeqCst), expected_calls);
    old_pin.release()?;
    Ok(())
}

#[test]
fn text_bridge_same_bytes_rename_rebinds_without_second_provider_call() -> TestResult {
    let directory = tempfile::tempdir()?;
    let calls = Arc::new(AtomicUsize::new(0));
    let bridge = Arc::new(TextEmbeddingBridge::new(FixedTextProvider {
        calls: Arc::clone(&calls),
    })?);
    let recipe = bridge.recipe(1024)?;
    run_rename(directory.path(), bridge, &recipe, &calls, 1, 1.0)
}

#[test]
fn text_bridge_changed_payload_reembeds_instead_of_rebinding_old_vector() -> TestResult {
    let directory = tempfile::tempdir()?;
    let calls = Arc::new(AtomicUsize::new(0));
    let bridge = Arc::new(TextEmbeddingBridge::new(FixedTextProvider {
        calls: Arc::clone(&calls),
    })?);
    let recipe = bridge.recipe(1024)?;
    let (kernel, objects, catalog) = kernel(directory.path(), bridge, recipe.clone())?;
    let configured = configure(&kernel, &recipe)?;
    let (first, a_record) = advance(&kernel, configured, 2, 1, "a.rs", b"alpha", None)?;
    let first = materialize(&kernel, first, 3)?;
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let (changed, b_record) = advance(&kernel, first, 4, 2, "b.rs", b"beta", Some(a_record))?;
    let a: SourceRecord = serde_json::from_slice(&objects.get(a_record, &mut budget())?.payload)?;
    let b: SourceRecord = serde_json::from_slice(&objects.get(b_record, &mut budget())?.payload)?;
    assert_ne!(a.bytes, b.bytes);
    let pending = result_at(&catalog, &objects, &recipe, changed)?;
    assert!(pending.hits.is_empty());
    assert_eq!(pending.traversal, NativeSemanticTraversal::ExactExhausted);
    assert!(!pending.exhaustive);
    let changed = materialize(&kernel, changed, 5)?;
    assert_eq!(
        member(&kernel, &objects, changed)?.input.source,
        Some(b_record)
    );
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        cosine_at(&catalog, &objects, &recipe, changed)?.to_bits(),
        1.0f64.to_bits()
    );
    Ok(())
}

#[test]
fn generic_path_sensitive_provider_reembeds_same_bytes_rename() -> TestResult {
    let directory = tempfile::tempdir()?;
    let calls = Arc::new(AtomicUsize::new(0));
    let recipe = EmbeddingRecipe {
        input_policy: EmbeddingInputPolicy::SourceUtf8,
        model_identity: Digest([21; 32]),
        dimension: 2,
        normalization: VectorNormalization::L2Unit,
        chunk_policy: None,
        output_policy: Digest([22; 32]),
        max_vectors: 1,
        max_source_bytes: 1024,
    };
    run_rename(
        directory.path(),
        PathSensitiveProvider {
            calls: Arc::clone(&calls),
            recipe: recipe.clone(),
        },
        &recipe,
        &calls,
        2,
        0.0,
    )
}

#[test]
fn stateful_text_provider_cannot_reuse_identical_payload_without_opt_in() -> TestResult {
    let directory = tempfile::tempdir()?;
    let calls = Arc::new(AtomicUsize::new(0));
    let bridge = Arc::new(TextEmbeddingBridge::new(StatefulTextProvider {
        calls: Arc::clone(&calls),
    })?);
    let recipe = bridge.recipe(1024)?;
    run_rename(directory.path(), bridge, &recipe, &calls, 2, 0.0)
}

#[test]
fn generic_artifact_cannot_be_reused_after_switching_to_text_bridge() -> TestResult {
    let directory = tempfile::tempdir()?;
    let generic_calls = Arc::new(AtomicUsize::new(0));
    let bridge_calls = Arc::new(AtomicUsize::new(0));
    let bridge = Arc::new(TextEmbeddingBridge::new(FixedTextProvider {
        calls: Arc::clone(&bridge_calls),
    })?);
    let recipe = bridge.recipe(1024)?;
    let (initial_kernel, initial_objects, initial_catalog) = kernel(
        directory.path(),
        PathSensitiveProvider {
            calls: Arc::clone(&generic_calls),
            recipe: recipe.clone(),
        },
        recipe.clone(),
    )?;
    let configured = configure(&initial_kernel, &recipe)?;
    let (first, a_record) = advance(&initial_kernel, configured, 2, 1, "a.rs", b"alpha", None)?;
    let first = materialize(&initial_kernel, first, 3)?;
    assert_eq!(generic_calls.load(Ordering::SeqCst), 1);
    assert_eq!(bridge_calls.load(Ordering::SeqCst), 0);
    let (renamed, b_record) = advance(
        &initial_kernel,
        first,
        4,
        2,
        "b.rs",
        b"alpha",
        Some(a_record),
    )?;
    drop(initial_kernel);
    drop(initial_objects);
    drop(initial_catalog);

    let (reopened, objects, catalog) = kernel(directory.path(), bridge, recipe.clone())?;
    let renamed = materialize(&reopened, renamed, 5)?;
    assert_eq!(
        member(&reopened, &objects, renamed)?.input.source,
        Some(b_record)
    );
    assert_eq!(bridge_calls.load(Ordering::SeqCst), 1);
    assert_eq!(generic_calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        cosine_at(&catalog, &objects, &recipe, renamed)?.to_bits(),
        1.0f64.to_bits()
    );
    Ok(())
}

#[test]
fn text_bridge_reuses_same_payload_after_restart_without_old_pin() -> TestResult {
    let directory = tempfile::tempdir()?;
    let calls = Arc::new(AtomicUsize::new(0));
    let bridge = Arc::new(TextEmbeddingBridge::new(FixedTextProvider {
        calls: Arc::clone(&calls),
    })?);
    let recipe = bridge.recipe(1024)?;
    let (initial_kernel, initial_objects, initial_catalog) =
        kernel(directory.path(), Arc::clone(&bridge), recipe.clone())?;
    let configured = configure(&initial_kernel, &recipe)?;
    let (first, a_record) = advance(&initial_kernel, configured, 2, 1, "a.rs", b"alpha", None)?;
    let first = materialize(&initial_kernel, first, 3)?;
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let (renamed, b_record) = advance(
        &initial_kernel,
        first,
        4,
        2,
        "b.rs",
        b"alpha",
        Some(a_record),
    )?;
    let pending = result_at(&initial_catalog, &initial_objects, &recipe, renamed)?;
    assert!(pending.hits.is_empty());
    assert_eq!(pending.traversal, NativeSemanticTraversal::ExactExhausted);
    assert!(!pending.exhaustive);
    drop(initial_kernel);
    drop(initial_objects);
    drop(initial_catalog);

    let (reopened, objects, catalog) = kernel(directory.path(), bridge, recipe.clone())?;
    let renamed = materialize(&reopened, renamed, 5)?;
    assert_eq!(
        member(&reopened, &objects, renamed)?.input.source,
        Some(b_record)
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        cosine_at(&catalog, &objects, &recipe, renamed)?.to_bits(),
        1.0f64.to_bits()
    );
    Ok(())
}

struct ChunkLifecycleProvider {
    calls: Arc<AtomicUsize>,
    batches: Arc<std::sync::Mutex<Vec<Vec<String>>>>,
}

impl TextEmbeddingProvider for ChunkLifecycleProvider {
    fn embed_batch(
        &self,
        texts: &[&str],
    ) -> Result<quanta_index_core::TextEmbeddingBatch, CoreError> {
        let vectors = texts
            .iter()
            .map(|text| match *text {
                "🙂" | "zzzz" => vec![0.0, 1.0],
                "B" => vec![0.6, 0.8],
                "AB" => vec![0.8, 0.6],
                _ => vec![1.0, 0.0],
            })
            .collect();
        Ok(quanta_index_core::TextEmbeddingBatch::unscoped(vectors))
    }

    fn embed_batch_within(
        &self,
        texts: &[&str],
        budget: &RequestBudgetV1,
    ) -> Result<quanta_index_core::TextEmbeddingBatch, CoreError> {
        budget.checkpoint("semantic:chunk-lifecycle")?;
        self.batches
            .lock()
            .map_err(|_error| CoreError::InvalidContract("chunk batch lock".into()))?
            .push(texts.iter().map(|text| (*text).to_owned()).collect());
        let _previous = self.calls.fetch_add(1, Ordering::SeqCst);
        let memory = quanta_index_core::TextEmbeddingBatch::reserve_output(
            texts.len(),
            self.dimension(),
            budget,
        )?;
        Ok(quanta_index_core::TextEmbeddingBatch::from_parts(
            self.embed_batch(texts)?.into_unscoped()?,
            memory,
        ))
    }

    fn model_id(&self) -> &'static str {
        "chunk-lifecycle-model"
    }

    fn model_revision(&self) -> &'static str {
        "fixed-v1"
    }

    fn dimension(&self) -> usize {
        2
    }

    fn normalization(&self) -> EmbeddingNormalization {
        EmbeddingNormalization::L2Unit
    }

    fn supports_deterministic_payload_reuse(&self) -> bool {
        true
    }
}

fn chunk_owner(byte: u8) -> OwnerKey {
    OwnerKey {
        producer: PRODUCER,
        stable_id: StableOwnerId([byte; 16]),
    }
}

fn chunk_advance(
    kernel: &Kernel<SqliteCatalog>,
    view: ViewRef,
    operation_sequence: u64,
    event_sequence: u64,
    owner: OwnerKey,
    input: Option<(&str, &[u8])>,
    previous: Option<ObjectRef>,
) -> TestResult<(ViewRef, Option<ObjectRef>)> {
    let key = operation(operation_sequence);
    let ingress = kernel.begin_input(key)?;
    let next = input
        .map(|(path, bytes)| {
            kernel.stage_source_text(
                ingress,
                path,
                bytes,
                SourceTextAdmission::Utf8 {
                    policy: Digest([8; 32]),
                },
                &mut budget(),
            )
        })
        .transpose()?;
    let owners = kernel.stage_mutations(
        ingress,
        std::iter::once(Ok(OwnerMutation {
            owner,
            expected: previous,
            next,
        })),
        &mut budget(),
    )?;
    let mut transition = SourceTransition {
        expected_source: source(kernel, view)?,
        owners,
        events: Vec::new(),
    };
    transition.events.push(SourceEvent {
        producer: PRODUCER,
        stream: StreamId([5; 16]),
        stream_epoch: 1,
        predecessor: event_sequence
            .checked_sub(1)
            .ok_or("chunk event sequence must start at one")?,
        sequence: event_sequence,
        payload: transition.owner_digest()?,
    });
    let next_view = published(block_on(kernel.submit(
        UpdateRequest {
            operation: key,
            expected_view: view,
            kind: UpdateKind::SourceAdvance(transition),
        },
        &mut budget(),
    ))?)?;
    Ok((next_view, next))
}

fn chunk_member_for(
    kernel: &Kernel<SqliteCatalog>,
    objects: &FileObjectStore,
    view: ViewRef,
    owner: OwnerKey,
) -> TestResult<OwnerMemberBinding> {
    let ticket = kernel.pin_view(ViewSelection::Exact(view), &budget())?;
    let mut key = b"member\0".to_vec();
    key.extend_from_slice(&INSTANCE.0);
    let coverage_ref = objects
        .lookup_object(ticket.view().coverage, &key, &mut budget())?
        .ok_or("chunk coverage absent")?;
    let coverage: CoverageBinding =
        serde_json::from_slice(&objects.get(coverage_ref, &mut budget())?.payload)?;
    let owner_ref = objects
        .lookup_object(coverage.owners, &owner_map_key(owner), &mut budget())?
        .ok_or("chunk owner coverage absent")?;
    let owner_coverage: OwnerCoverage =
        serde_json::from_slice(&objects.get(owner_ref, &mut budget())?.payload)?;
    let member_ref = owner_coverage
        .searchable
        .ok_or("chunk owner member absent")?;
    Ok(serde_json::from_slice(
        &objects.get(member_ref, &mut budget())?.payload,
    )?)
}

fn chunk_artifact_members(
    objects: &FileObjectStore,
    member: &OwnerMemberBinding,
) -> TestResult<Vec<quanta_index_semantic::native::ChunkMember>> {
    let artifact = objects.get(member.artifact, &mut budget())?;
    let value: serde_json::Value = serde_json::from_slice(&artifact.payload)?;
    Ok(serde_json::from_value(
        value
            .get("chunks")
            .ok_or("chunk artifact lacks members")?
            .clone(),
    )?)
}

fn chunk_result_at(
    catalog: &SqliteCatalog,
    objects: &FileObjectStore,
    recipe: &EmbeddingRecipe,
    view: ViewRef,
) -> TestResult<SearchResult> {
    let mut work = budget();
    let mut read = ReadContext {
        objects,
        maps: objects,
        budget: &mut work,
    };
    let session = catalog.open_read_session()?;
    Ok(search_exact_acquired(
        catalog,
        session.as_ref(),
        Duration::from_secs(30),
        TicketSearchQuery {
            selection: ViewSelection::Exact(view),
            instance: INSTANCE,
            format: FORMAT,
            recipe,
            vector: &[1.0, 0.0],
            top_k: 8,
            page_size: 8,
            max_owners: 8,
        },
        &mut read,
    )?)
}

#[test]
fn chunk_members_survive_owner_local_delta_delete_old_pin_and_current_reopen() -> TestResult {
    let directory = tempfile::tempdir()?;
    let calls = Arc::new(AtomicUsize::new(0));
    let batches = Arc::new(std::sync::Mutex::new(Vec::new()));
    let bridge = Arc::new(TextEmbeddingBridge::with_utf8_chunks(
        ChunkLifecycleProvider {
            calls: Arc::clone(&calls),
            batches: Arc::clone(&batches),
        },
        quanta_index_semantic::native::Utf8ChunkPolicy { max_utf8_bytes: 4 },
    )?);
    let recipe = bridge.chunk_recipe(1024, 4)?;
    let (kernel, objects, catalog) = kernel(directory.path(), Arc::clone(&bridge), recipe.clone())?;
    let configured = configure(&kernel, &recipe)?;
    let owner_a = owner();
    let owner_b = chunk_owner(7);
    let (a_pending, a_record) = chunk_advance(
        &kernel,
        configured,
        2,
        1,
        owner_a,
        Some(("a.rs", "Aé\n🙂B".as_bytes())),
        None,
    )?;
    let a_record = a_record.ok_or("first chunk source missing")?;
    let a_complete = materialize(&kernel, a_pending, 3)?;
    let (b_pending, b_record) = chunk_advance(
        &kernel,
        a_complete,
        4,
        2,
        owner_b,
        Some(("b.rs", b"zzzz")),
        None,
    )?;
    let b_record = b_record.ok_or("second chunk source missing")?;
    let both = materialize(&kernel, b_pending, 5)?;
    let old_pin = kernel.pin_view(ViewSelection::Exact(both), &budget())?;
    let a_before = chunk_member_for(&kernel, &objects, both, owner_a)?;
    let b_before = chunk_member_for(&kernel, &objects, both, owner_b)?;
    let a_members_before = chunk_artifact_members(&objects, &a_before)?;
    let b_members_before = chunk_artifact_members(&objects, &b_before)?;
    assert_eq!(a_members_before.len(), 3);
    assert_eq!(b_members_before.len(), 1);
    assert_eq!(
        a_members_before
            .iter()
            .map(quanta_index_semantic::native::ChunkMember::payload_span)
            .collect::<Result<Vec<_>, _>>()?,
        [(0, 4), (4, 8), (8, 9)]
    );
    let old_result = chunk_result_at(&catalog, &objects, &recipe, both)?;
    assert_eq!(old_result.hits.len(), 4);
    assert_eq!(
        old_result
            .hits
            .iter()
            .filter(|hit| hit.owner == owner_a)
            .count(),
        3
    );
    assert!(old_result.hits.iter().all(|hit| hit.chunk.is_some()));

    let (a_changed_pending, a_changed_record) = chunk_advance(
        &kernel,
        both,
        6,
        3,
        owner_a,
        Some(("a.rs", "Aé\nAB".as_bytes())),
        Some(a_record),
    )?;
    let a_changed_record = a_changed_record.ok_or("changed chunk source missing")?;
    assert_ne!(a_record, a_changed_record);
    let pending_result = chunk_result_at(&catalog, &objects, &recipe, a_changed_pending)?;
    assert!(pending_result.hits.iter().all(|hit| hit.owner != owner_a));
    assert_eq!(
        chunk_result_at(&catalog, &objects, &recipe, both)?
            .hits
            .len(),
        4
    );
    let changed = materialize(&kernel, a_changed_pending, 7)?;
    let a_after = chunk_member_for(&kernel, &objects, changed, owner_a)?;
    let b_after = chunk_member_for(&kernel, &objects, changed, owner_b)?;
    let a_members_after = chunk_artifact_members(&objects, &a_after)?;
    assert_ne!(a_before.artifact, a_after.artifact);
    assert_eq!(b_before.artifact, b_after.artifact);
    assert_eq!(
        chunk_artifact_members(&objects, &b_after)?,
        b_members_before
    );
    assert_eq!(a_members_after.len(), 2);
    let first_before = a_members_before.first().ok_or("old first chunk missing")?;
    let first_after = a_members_after.first().ok_or("new first chunk missing")?;
    let second_after = a_members_after.get(1).ok_or("new second chunk missing")?;
    assert_eq!(first_after.id, first_before.id);
    assert_eq!(first_after.payload_span()?, (0, 4));
    assert_eq!(second_after.payload_span()?, (4, 6));
    let changed_result = chunk_result_at(&catalog, &objects, &recipe, changed)?;
    assert_eq!(changed_result.hits.len(), 3);
    assert!(changed_result.hits.iter().all(|hit| hit.chunk.is_some()));
    assert_eq!(calls.load(Ordering::SeqCst), 3);
    assert_eq!(
        batches
            .lock()
            .map_err(|_error| "chunk batch lock")?
            .as_slice(),
        &[
            vec!["Aé\n".to_owned(), "🙂".to_owned(), "B".to_owned()],
            vec!["zzzz".to_owned()],
            vec!["AB".to_owned()],
        ]
    );

    let (deleted, deleted_record) = chunk_advance(
        &kernel,
        changed,
        8,
        4,
        owner_a,
        None,
        Some(a_changed_record),
    )?;
    assert_eq!(deleted_record, None);
    let deleted_result = chunk_result_at(&catalog, &objects, &recipe, deleted)?;
    assert!(deleted_result.hits.iter().all(|hit| hit.owner == owner_b));
    assert_eq!(
        chunk_result_at(&catalog, &objects, &recipe, both)?
            .hits
            .len(),
        4
    );
    assert_eq!(old_pin.view().identity, both);
    old_pin.release()?;
    drop(kernel);
    drop(objects);
    drop(catalog);

    let (reopened, reopened_objects, reopened_catalog) =
        self::kernel(directory.path(), bridge, recipe.clone())?;
    assert_eq!(
        chunk_member_for(&reopened, &reopened_objects, deleted, owner_b)?.artifact,
        b_before.artifact
    );
    let reopened_result = chunk_result_at(&reopened_catalog, &reopened_objects, &recipe, deleted)?;
    assert!(reopened_result.hits.iter().all(|hit| hit.owner == owner_b));
    assert_eq!(calls.load(Ordering::SeqCst), 3);
    assert_eq!(
        chunk_member_for(&reopened, &reopened_objects, both, owner_a)?.artifact,
        a_before.artifact
    );
    assert_eq!(
        chunk_result_at(&reopened_catalog, &reopened_objects, &recipe, both)?
            .hits
            .len(),
        4
    );
    assert_eq!(b_record, b_after.input.source.ok_or("B source missing")?);
    Ok(())
}
