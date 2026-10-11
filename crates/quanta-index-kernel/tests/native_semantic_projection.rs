//! Real catalog/storage projection-to-semantic materialization and acquired read.
use std::fmt::Write as _;
use std::{
    error::Error,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use futures::executor::block_on;
use quanta_index_catalog::SqliteCatalog;
use quanta_index_core::index::{
    CollectionId, Digest, GroupActivation, IndexInstanceId, IndexKind, IndexProfile, IngressRef,
    InputDomain, InstanceSpec, MapRoot, ObjectRef, OperationKey, OperationStatus, OwnerKey,
    OwnerMutation, OwnerMutationSet, ProducerCompleteness, ProducerId, ProjectionBinding,
    ProjectionCertificate, ProjectionId, ProjectionOwnerContribution, ProjectionTransition,
    PublicationGroupId, SchemaId, SlotId, SourceCut, SourceEvent, SourceRecord, SourceTransition,
    StableOwnerId, StorageFormatId, StreamId, SubmitOutcome, TenantId, UpdateKind, UpdateRequest,
    ViewRef, ViewSelection, WorkBudget, WorkLimits, WorkScope, WorkspaceKey, WorktreeId,
    ports::{ImmutableObjectPort, IndexFuture, InputStagingPort, PersistentMapPort},
};
use quanta_index_kernel::{AdapterDescriptor, AdapterRegistry, Kernel, KernelServices};
use quanta_index_semantic::native::{
    EmbeddingInput, EmbeddingInputPolicy, EmbeddingProvider, EmbeddingRecipe, EmbeddingVector,
    NativeSemantic, ReadContext, TicketSearchQuery, VectorNormalization, search_exact_acquired,
};
use quanta_index_storage::FileObjectStore;

type TestResult<T = ()> = Result<T, Box<dyn Error>>;
const SOURCE: ProducerId = ProducerId([51; 16]);
const RUNTIME: ProducerId = ProducerId([52; 16]);
const PROJECTION: ProjectionId = ProjectionId([53; 16]);
const INSTANCE: IndexInstanceId = IndexInstanceId([54; 16]);
const SCHEMA: SchemaId = SchemaId([55; 16]);
const FORMAT: StorageFormatId = StorageFormatId([56; 16]);
const KIND: IndexKind = IndexKind([57; 16]);

fn budget() -> WorkBudget {
    WorkBudget::new(
        WorkLimits {
            reads: 1_000_000,
            writes: 1_000_000,
            bytes: 128 * 1024 * 1024,
            mutations: 1_000_000,
        },
        Instant::now() + Duration::from_secs(30),
    )
}
fn workspace() -> WorkspaceKey {
    WorkspaceKey {
        tenant: TenantId([1; 16]),
        collection: CollectionId([2; 16]),
        worktree: WorktreeId([3; 16]),
        slot: SlotId([4; 16]),
    }
}
fn owner(producer: ProducerId) -> OwnerKey {
    OwnerKey {
        producer,
        stable_id: StableOwnerId([9; 16]),
    }
}
fn operation(producer: ProducerId, stream: u8, sequence: u64) -> OperationKey {
    OperationKey {
        workspace: workspace(),
        epoch: 1,
        producer,
        stream: StreamId([stream; 16]),
        stream_epoch: 1,
        sequence,
    }
}
fn published(outcome: SubmitOutcome) -> TestResult<ViewRef> {
    match outcome {
        SubmitOutcome::Terminal(OperationStatus::Published(view)) => Ok(view),
        other => Err(format!("publication failed: {other:?}").into()),
    }
}
fn current(kernel: &Kernel<SqliteCatalog>) -> TestResult<quanta_index_core::index::CatalogView> {
    Ok(kernel
        .pin_view(ViewSelection::Latest {
            workspace: workspace(),
            epoch: 1,
        })?
        .view()
        .clone())
}
fn binding(objects: &FileObjectStore, root: MapRoot) -> TestResult<ProjectionBinding> {
    let mut key = b"projection\0".to_vec();
    key.extend_from_slice(&PROJECTION.0);
    let reference = objects
        .lookup(root, &key, &mut budget())?
        .ok_or("projection absent")?;
    Ok(serde_json::from_slice(
        &objects.get(reference, &mut budget())?.payload,
    )?)
}

struct FixedProvider {
    calls: Arc<AtomicUsize>,
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
    fn embed<'a>(&'a self, input: EmbeddingInput<'a>) -> IndexFuture<'a, Vec<EmbeddingVector>> {
        let _previous = self.calls.fetch_add(1, Ordering::SeqCst);
        let selected = if input.source_bytes == b"alpha" {
            Ok(vec![1.0, 0.0])
        } else if input.source_bytes == b"beta" {
            Ok(vec![0.0, 1.0])
        } else {
            Err(quanta_index_core::index::IndexError::InvalidInput(
                "unrecognized fixed input".into(),
            ))
        };
        Box::pin(async move {
            selected.map(|values| {
                vec![EmbeddingVector {
                    id: b"whole-file".to_vec(),
                    values,
                }]
            })
        })
    }
}

fn stage_source(objects: &FileObjectStore) -> TestResult<ObjectRef> {
    // Legacy physical-prestage fixture, adopted under ingress before admit.
    let bytes = objects.put(b"B stable", &[], &mut budget())?.reference;
    let record = SourceRecord {
        path: "b.rs".into(),
        bytes,
        byte_len: 8,
    };
    Ok(objects
        .put(&serde_json::to_vec(&record)?, &[bytes], &mut budget())?
        .reference)
}

fn adopt_fixture_tree(
    catalog: &SqliteCatalog,
    objects: &FileObjectStore,
    ingress: IngressRef,
    reference: ObjectRef,
    seen: &mut std::collections::BTreeSet<ObjectRef>,
) -> TestResult {
    if !seen.insert(reference) {
        return Ok(());
    }
    let stored = objects.get(reference, &mut budget())?;
    for child in stored.registration.children.iter().copied() {
        adopt_fixture_tree(catalog, objects, ingress, child, seen)?;
    }
    catalog.reserve_inputs(ingress, std::slice::from_ref(&stored.registration))?;
    catalog.confirm_inputs(ingress, std::slice::from_ref(&stored.registration))?;
    Ok(())
}

fn stage_manifest(
    kernel: &Kernel<SqliteCatalog>,
    catalog: &SqliteCatalog,
    objects: &FileObjectStore,
    key: OperationKey,
    owners: Vec<OwnerMutation>,
) -> TestResult<OwnerMutationSet> {
    let ingress = kernel.begin_input(key)?;
    let mut seen = std::collections::BTreeSet::new();
    for reference in owners.iter().filter_map(|mutation| mutation.next) {
        adopt_fixture_tree(catalog, objects, ingress, reference, &mut seen)?;
    }
    Ok(kernel.stage_mutations(ingress, owners.into_iter().map(Ok), &mut budget())?)
}

fn source_request(
    kernel: &Kernel<SqliteCatalog>,
    catalog: &SqliteCatalog,
    objects: &FileObjectStore,
    view: ViewRef,
    cut: SourceCut,
    record: ObjectRef,
) -> TestResult<UpdateRequest> {
    let key = operation(SOURCE, 60, 2);
    let mut transition = SourceTransition {
        expected_source: cut,
        owners: stage_manifest(
            kernel,
            catalog,
            objects,
            key,
            vec![OwnerMutation {
                owner: owner(SOURCE),
                expected: None,
                next: Some(record),
            }],
        )?,
        events: vec![],
    };
    transition.events.push(SourceEvent {
        producer: SOURCE,
        stream: StreamId([60; 16]),
        stream_epoch: 1,
        predecessor: 0,
        sequence: 1,
        payload: transition.owner_digest()?,
    });
    Ok(UpdateRequest {
        operation: key,
        expected_view: view,
        kind: UpdateKind::SourceAdvance(transition),
    })
}
fn stage_contribution(
    objects: &FileObjectStore,
    source: ObjectRef,
    text: &[u8],
    stamp: u8,
) -> TestResult<ObjectRef> {
    let output = objects.put(text, &[], &mut budget())?.reference;
    let contribution = ProjectionOwnerContribution {
        projection: PROJECTION,
        producer: RUNTIME,
        owner: owner(RUNTIME),
        source_owner: Some(owner(SOURCE)),
        source: Some(source),
        dependency_stamp: Digest([stamp; 32]),
        output: Some(output),
    };
    Ok(objects
        .put(
            &serde_json::to_vec(&contribution)?,
            &contribution.object_children(),
            &mut budget(),
        )?
        .reference)
}
fn projection_request(
    kernel: &Kernel<SqliteCatalog>,
    catalog: &SqliteCatalog,
    objects: &FileObjectStore,
    view: ViewRef,
    source: SourceCut,
    prior: Option<ProjectionBinding>,
    sequence: u64,
    expected: Option<ObjectRef>,
    next: ObjectRef,
) -> TestResult<UpdateRequest> {
    let key = operation(RUNTIME, 63, sequence);
    let affected = stage_manifest(
        kernel,
        catalog,
        objects,
        key,
        vec![OwnerMutation {
            owner: owner(RUNTIME),
            expected,
            next: Some(next),
        }],
    )?;
    let mut transition = ProjectionTransition {
        projection: PROJECTION,
        producer: RUNTIME,
        expected: prior,
        target_source: source,
        schema: SCHEMA,
        producer_policy: Digest([61; 32]),
        dependency_context: Digest([62; 32]),
        completeness: ProducerCompleteness::Partial {
            missing: MapRoot(None),
        },
        owners: affected,
        events: vec![],
    };
    let certificate = ProjectionCertificate {
        projection: PROJECTION,
        producer: RUNTIME,
        prior,
        target_source: source,
        schema: SCHEMA,
        producer_policy: transition.producer_policy,
        dependency_context: transition.dependency_context,
        affected,
    };
    let ingress = kernel.begin_input(key)?;
    let certificate = kernel.stage_projection_certificate(ingress, &certificate, &mut budget())?;
    transition.completeness = ProducerCompleteness::CompleteAffectedDomain { certificate };
    transition.events.push(SourceEvent {
        producer: RUNTIME,
        stream: StreamId([63; 16]),
        stream_epoch: 1,
        predecessor: sequence - 1,
        sequence,
        payload: transition.payload_digest()?,
    });
    Ok(UpdateRequest {
        operation: key,
        expected_view: view,
        kind: UpdateKind::ProjectionAdvance(Box::new(transition)),
    })
}

#[test]
fn changed_projection_output_reembeds_and_cold_resume_reuses_committed_receipt() -> TestResult {
    let directory = tempfile::tempdir()?;
    let objects = Arc::new(FileObjectStore::open(directory.path().join("objects"))?);
    let catalog = Arc::new(SqliteCatalog::open(
        directory.path(),
        Duration::from_millis(100),
    )?);
    let recipe = EmbeddingRecipe {
        input_policy: EmbeddingInputPolicy::ProjectionUtf8 {
            projection: PROJECTION,
            producer: RUNTIME,
            schema: SCHEMA,
        },
        model_identity: Digest([70; 32]),
        dimension: 2,
        normalization: VectorNormalization::L2Unit,
        output_policy: Digest([71; 32]),
        max_vectors: 1,
        max_source_bytes: 1024,
    };
    let calls = Arc::new(AtomicUsize::new(0));
    let adapter = NativeSemantic::new(
        FixedProvider {
            calls: Arc::clone(&calls),
            recipe: recipe.clone(),
        },
        recipe.clone(),
    )?;
    let spec = InstanceSpec {
        instance: INSTANCE,
        kind: KIND,
        group: PublicationGroupId([72; 16]),
        activation: GroupActivation::AsyncCoverage,
        input_domain: InputDomain::Projection {
            projection: PROJECTION,
            producer: RUNTIME,
        },
        schema: SCHEMA,
        policy: recipe.fingerprint(),
        format: FORMAT,
    };
    let mut registry = AdapterRegistry::new();
    registry.register_owner(
        AdapterDescriptor {
            kind: KIND,
            schema: SCHEMA,
            format: FORMAT,
        },
        adapter,
    )?;
    let kernel = Kernel::new(
        KernelServices::new(objects.clone(), objects.clone(), catalog.clone()),
        registry,
    );
    let genesis = kernel.create_workspace(workspace(), 1, &mut budget())?;
    let ingress = kernel.begin_input(operation(SOURCE, 60, 1))?;
    let profile = kernel.stage_profile(
        ingress,
        &IndexProfile {
            instances: vec![spec],
        },
        &mut budget(),
    )?;
    let configured = published(block_on(kernel.submit(
        UpdateRequest {
            operation: operation(SOURCE, 60, 1),
            expected_view: genesis,
            kind: UpdateKind::Reconfigure { profile },
        },
        &mut budget(),
    ))?)?;
    let source_record = stage_source(&objects)?;
    let sourced = published(block_on(kernel.submit(
        source_request(
            &kernel,
            &catalog,
            &objects,
            configured,
            current(&kernel)?.source,
            source_record,
        )?,
        &mut budget(),
    ))?)?;
    let source_cut = current(&kernel)?.source;
    let first_contribution = stage_contribution(&objects, source_record, b"alpha", 1)?;
    let first = published(block_on(kernel.submit(
        projection_request(
            &kernel,
            &catalog,
            &objects,
            sourced,
            source_cut,
            None,
            1,
            None,
            first_contribution,
        )?,
        &mut budget(),
    ))?)?;
    let first_binding = binding(&objects, current(&kernel)?.projections)?;
    let first_intent = kernel
        .ready_work(workspace(), 1, 10, &mut budget())?
        .into_iter()
        .next()
        .ok_or("first work absent")?;
    let first_claim = kernel.claim_work(&first_intent, Duration::from_secs(10))?;
    let materialized_first = published(block_on(kernel.submit(
        UpdateRequest {
            operation: operation(SOURCE, 60, 3),
            expected_view: first,
            kind: UpdateKind::Materialize { claim: first_claim },
        },
        &mut budget(),
    ))?)?;
    let query = |selection| TicketSearchQuery {
        selection,
        instance: INSTANCE,
        format: FORMAT,
        recipe: &recipe,
        vector: &[1.0, 0.0],
        top_k: 1,
        page_size: 8,
        max_owners: 8,
    };
    let mut read_budget = budget();
    let mut read = ReadContext {
        objects: objects.as_ref(),
        maps: objects.as_ref(),
        budget: &mut read_budget,
    };
    let first_result = search_exact_acquired(
        catalog.as_ref(),
        query(ViewSelection::Exact(materialized_first)),
        &mut read,
    )?;
    assert_eq!(first_result.hits.len(), 1);
    assert_eq!(first_result.hits[0].cosine, 1.0);
    let second_contribution = stage_contribution(&objects, source_record, b"beta", 2)?;
    let second = published(block_on(kernel.submit(
        projection_request(
            &kernel,
            &catalog,
            &objects,
            materialized_first,
            current(&kernel)?.source,
            Some(first_binding),
            2,
            Some(first_contribution),
            second_contribution,
        )?,
        &mut budget(),
    ))?)?;
    assert_eq!(current(&kernel)?.source, source_cut);
    let second_intent = kernel
        .ready_work(workspace(), 1, 10, &mut budget())?
        .into_iter()
        .next()
        .ok_or("second work absent")?;
    let second_claim = kernel.claim_work(&second_intent, Duration::from_secs(10))?;
    let second_materialization = UpdateRequest {
        operation: operation(SOURCE, 60, 4),
        expected_view: second,
        kind: UpdateKind::Materialize {
            claim: second_claim,
        },
    };
    // Model a lost caller reply by discarding the returned value after the
    // catalog commit. This does not inject a transport delivery failure.
    let _ignored_reply = block_on(kernel.submit(second_materialization.clone(), &mut budget()))?;
    let committed_view = current(&kernel)?.identity;
    let latest = search_exact_acquired(
        catalog.as_ref(),
        query(ViewSelection::Latest {
            workspace: workspace(),
            epoch: 1,
        }),
        &mut read,
    )?;
    assert_eq!(latest.hits.len(), 1);
    assert_eq!(latest.hits[0].cosine, 0.0);
    let old = search_exact_acquired(
        catalog.as_ref(),
        query(ViewSelection::Exact(materialized_first)),
        &mut read,
    )?;
    assert_eq!(old.hits[0].cosine, 1.0);
    assert_eq!(calls.load(Ordering::SeqCst), 2);

    // The committed request body is no longer physically available. Its live
    // source and projection children remain owned by the published view.
    let WorkScope::Owner { expected, .. } = second_claim.intent.scope else {
        return Err("materialization must be owner-scoped".into());
    };
    let mut children: Vec<_> = [expected.source, expected.projection]
        .into_iter()
        .flatten()
        .collect();
    children.sort_unstable();
    children.dedup();
    let request_ref = objects
        .identify(
            &serde_json::to_vec(&second_materialization)?,
            &children,
            &mut budget(),
        )?
        .reference;
    let mut name = String::with_capacity(64);
    for byte in (request_ref.0).0 {
        write!(&mut name, "{byte:02x}")?;
    }
    let shard = format!("{:02x}", (request_ref.0).0.first().ok_or("digest empty")?);
    drop(read);
    drop(kernel);
    drop(catalog);
    drop(objects);
    std::fs::remove_file(
        directory
            .path()
            .join("objects/objects")
            .join(shard)
            .join(name),
    )?;

    let objects = Arc::new(FileObjectStore::open(directory.path().join("objects"))?);
    let catalog = Arc::new(SqliteCatalog::open(
        directory.path(),
        Duration::from_millis(100),
    )?);
    let adapter = NativeSemantic::new(
        FixedProvider {
            calls: Arc::clone(&calls),
            recipe: recipe.clone(),
        },
        recipe.clone(),
    )?;
    let mut registry = AdapterRegistry::new();
    registry.register_owner(
        AdapterDescriptor {
            kind: KIND,
            schema: SCHEMA,
            format: FORMAT,
        },
        adapter,
    )?;
    let reopened = Kernel::new(
        KernelServices::new(objects.clone(), objects.clone(), catalog.clone()),
        registry,
    );
    assert_eq!(
        reopened.status(second_materialization.operation)?,
        OperationStatus::Published(committed_view)
    );
    assert_eq!(
        block_on(reopened.resume(second_materialization.operation, &mut budget()))?,
        SubmitOutcome::Terminal(OperationStatus::Published(committed_view))
    );
    assert_eq!(current(&reopened)?.identity, committed_view);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    let mut recovered_budget = budget();
    let mut recovered_read = ReadContext {
        objects: objects.as_ref(),
        maps: objects.as_ref(),
        budget: &mut recovered_budget,
    };
    let recovered = search_exact_acquired(
        catalog.as_ref(),
        query(ViewSelection::Exact(committed_view)),
        &mut recovered_read,
    )?;
    assert_eq!(recovered.hits.len(), 1);
    assert_eq!(recovered.hits[0].cosine, 0.0);
    Ok(())
}
