//! Producer-certified projection intake against real custody, map and catalog.
use futures::executor::block_on;
use quanta_index_catalog::SqliteCatalog;
use quanta_index_core::index::{
    AdmissionOutcome, AttemptContext, CollectionId, CoverageBinding, CoverageStatus, Digest,
    GroupActivation, ImmutableOperation, IndexError, IndexInstanceId, IndexKind, IndexProfile,
    IngressRef, InputDomain, InstanceSpec, MapRoot, ObjectRef, OperationKey, OperationStatus,
    OwnerCoverage, OwnerInputBinding, OwnerKey, OwnerMemberBinding, OwnerMutation,
    OwnerMutationSet, ProducerCompleteness, ProducerId, ProjectionBinding, ProjectionCertificate,
    ProjectionId, ProjectionOwnerContribution, ProjectionTransition, PublicationGroupId, SchemaId,
    SlotId, SourceCut, SourceEvent, SourceRecord, SourceTransition, StableOwnerId, StorageFormatId,
    StreamId, SubmitOutcome, TenantId, UpdateKind, UpdateRequest, ViewRef, ViewSelection,
    WorkBudget, WorkLimits, WorkspaceKey, WorktreeId,
    ports::{
        ImmutableObjectPort, IndexFuture, IndexOwnerBuildPort, InputStagingPort,
        OperationJournalPort, PersistentMapPort,
    },
};
use quanta_index_kernel::{
    AdapterDescriptor, AdapterRegistry, Kernel, KernelServices, OperationExecutionClass,
    ResumeDispatch,
};
use quanta_index_storage::FileObjectStore;
use std::{
    error::Error,
    fmt::Write as _,
    sync::Arc,
    time::{Duration, Instant},
};

type TestResult<T = ()> = Result<T, Box<dyn Error>>;

const SOURCE_PRODUCER: ProducerId = ProducerId([5; 16]);
const RUNTIME_PRODUCER: ProducerId = ProducerId([13; 16]);
const PROJECTION: ProjectionId = ProjectionId([14; 16]);
const INSTANCE: IndexInstanceId = IndexInstanceId([15; 16]);

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

fn source_operation(sequence: u64) -> OperationKey {
    OperationKey {
        workspace: workspace(),
        epoch: 1,
        producer: SOURCE_PRODUCER,
        stream: StreamId([6; 16]),
        stream_epoch: 1,
        sequence,
    }
}

fn runtime_operation(sequence: u64) -> OperationKey {
    OperationKey {
        workspace: workspace(),
        epoch: 1,
        producer: RUNTIME_PRODUCER,
        stream: StreamId([16; 16]),
        stream_epoch: 1,
        sequence,
    }
}

fn owner(producer: ProducerId, id: u8) -> OwnerKey {
    OwnerKey {
        producer,
        stable_id: StableOwnerId([id; 16]),
    }
}

fn spec() -> InstanceSpec {
    InstanceSpec {
        instance: INSTANCE,
        kind: IndexKind([17; 16]),
        group: PublicationGroupId([18; 16]),
        activation: GroupActivation::AsyncCoverage,
        input_domain: InputDomain::Projection {
            projection: PROJECTION,
            producer: RUNTIME_PRODUCER,
        },
        schema: SchemaId([19; 16]),
        policy: Digest([20; 32]),
        format: StorageFormatId([21; 16]),
    }
}

#[derive(Clone, Copy)]
struct Candidate {
    instance: IndexInstanceId,
    owner: OwnerKey,
    input: OwnerInputBinding,
    format: StorageFormatId,
}

struct ExactOwner;

impl IndexOwnerBuildPort for ExactOwner {
    type Candidate = Candidate;

    fn prepare_owner<'a>(
        &'a self,
        instance: &'a InstanceSpec,
        owner: OwnerKey,
        input: OwnerInputBinding,
        _: &'a mut AttemptContext<'_>,
    ) -> IndexFuture<'a, Candidate> {
        Box::pin(async move {
            Ok(Candidate {
                instance: instance.instance,
                owner,
                input,
                format: instance.format,
            })
        })
    }

    fn verify_owner<'a>(
        &'a self,
        candidate: Candidate,
        context: &'a mut AttemptContext<'_>,
    ) -> IndexFuture<'a, OwnerMemberBinding> {
        Box::pin(async move {
            let artifact = context.put(b"independent projection oracle artifact", &[])?;
            Ok(OwnerMemberBinding {
                instance: candidate.instance,
                owner: candidate.owner,
                input: candidate.input,
                format: candidate.format,
                artifact,
                logical_commitment: Digest([22; 32]),
            })
        })
    }
}

fn setup(
    root: &std::path::Path,
) -> TestResult<(
    Kernel<SqliteCatalog>,
    Arc<FileObjectStore>,
    Arc<SqliteCatalog>,
)> {
    let objects = Arc::new(FileObjectStore::open(root.join("objects"))?);
    let catalog = Arc::new(SqliteCatalog::open(root, Duration::from_millis(100))?);
    let mut registry = AdapterRegistry::new();
    registry.register_owner(
        AdapterDescriptor {
            kind: spec().kind,
            schema: spec().schema,
            format: spec().format,
        },
        ExactOwner,
    )?;
    registry.register_owner(
        AdapterDescriptor {
            kind: IndexKind([28; 16]),
            schema: SchemaId([29; 16]),
            format: StorageFormatId([30; 16]),
        },
        ExactOwner,
    )?;
    let services = KernelServices::new(objects.clone(), objects.clone(), catalog.clone());
    Ok((Kernel::new(services, registry), objects, catalog))
}

fn published(result: SubmitOutcome) -> TestResult<ViewRef> {
    match result {
        SubmitOutcome::Terminal(OperationStatus::Published(view)) => Ok(view),
        other => Err(format!("expected publication, got {other:?}").into()),
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

fn stage_source(objects: &FileObjectStore, path: &str, bytes: &[u8]) -> TestResult<ObjectRef> {
    // Legacy physical-prestage fixture. Catalog ingress adopts the closure
    // before admission; online GC is disabled for this native slice.
    let blob = objects.put(bytes, &[], &mut budget())?.reference;
    let record = SourceRecord {
        path: path.into(),
        bytes: blob,
        byte_len: u64::try_from(bytes.len())?,
    };
    Ok(objects
        .put(&serde_json::to_vec(&record)?, &[blob], &mut budget())?
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
    operation: OperationKey,
    owners: Vec<OwnerMutation>,
) -> TestResult<OwnerMutationSet> {
    let ingress = kernel.begin_input(operation)?;
    let mut seen = std::collections::BTreeSet::new();
    for reference in owners.iter().filter_map(|mutation| mutation.next) {
        adopt_fixture_tree(catalog, objects, ingress, reference, &mut seen)?;
    }
    Ok(kernel.stage_mutations(ingress, owners.into_iter().map(Ok), &mut budget())?)
}

fn stage_profile(
    kernel: &Kernel<SqliteCatalog>,
    key: OperationKey,
    instances: Vec<InstanceSpec>,
) -> TestResult<ObjectRef> {
    let ingress = kernel.begin_input(key)?;
    Ok(kernel.stage_profile(ingress, &IndexProfile { instances }, &mut budget())?)
}

fn source_request(
    kernel: &Kernel<SqliteCatalog>,
    catalog: &SqliteCatalog,
    objects: &FileObjectStore,
    view: ViewRef,
    source: SourceCut,
    operation_sequence: u64,
    event_sequence: u64,
    owners: Vec<OwnerMutation>,
) -> TestResult<UpdateRequest> {
    let operation = source_operation(operation_sequence);
    let mut transition = SourceTransition {
        expected_source: source,
        owners: stage_manifest(kernel, catalog, objects, operation, owners)?,
        events: Vec::new(),
    };
    transition.events.push(SourceEvent {
        producer: SOURCE_PRODUCER,
        stream: StreamId([6; 16]),
        stream_epoch: 1,
        predecessor: event_sequence
            .checked_sub(1)
            .ok_or("invalid event sequence")?,
        sequence: event_sequence,
        payload: transition.owner_digest()?,
    });
    Ok(UpdateRequest {
        operation,
        expected_view: view,
        kind: UpdateKind::SourceAdvance(transition),
    })
}

fn stage_contribution(
    objects: &FileObjectStore,
    source: Option<ObjectRef>,
    stamp: u8,
) -> TestResult<ObjectRef> {
    let output = objects.put(&[stamp], &[], &mut budget())?.reference;
    let contribution = ProjectionOwnerContribution {
        projection: PROJECTION,
        producer: RUNTIME_PRODUCER,
        owner: owner(RUNTIME_PRODUCER, 2),
        source_owner: source.map(|_| owner(SOURCE_PRODUCER, 2)),
        source,
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

fn stage_empty_contribution(objects: &FileObjectStore, stamp: u8) -> TestResult<ObjectRef> {
    let contribution = ProjectionOwnerContribution {
        projection: PROJECTION,
        producer: RUNTIME_PRODUCER,
        owner: owner(RUNTIME_PRODUCER, 2),
        source_owner: None,
        source: None,
        dependency_stamp: Digest([stamp; 32]),
        output: None,
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
    operation_sequence: u64,
    event_sequence: u64,
    schema: SchemaId,
    producer_policy: Digest,
    owners: Vec<OwnerMutation>,
) -> TestResult<UpdateRequest> {
    let operation = runtime_operation(operation_sequence);
    let affected = stage_manifest(kernel, catalog, objects, operation, owners)?;
    let mut transition = ProjectionTransition {
        projection: PROJECTION,
        producer: RUNTIME_PRODUCER,
        expected: prior,
        target_source: source,
        schema,
        producer_policy,
        dependency_context: Digest([25; 32]),
        completeness: ProducerCompleteness::Partial {
            missing: MapRoot(None),
        },
        owners: affected,
        events: Vec::new(),
    };
    let certificate = ProjectionCertificate {
        projection: transition.projection,
        producer: transition.producer,
        prior: transition.expected,
        target_source: transition.target_source,
        schema: transition.schema,
        producer_policy: transition.producer_policy,
        dependency_context: transition.dependency_context,
        affected,
    };
    let ingress = kernel.begin_input(operation)?;
    let certificate_ref =
        kernel.stage_projection_certificate(ingress, &certificate, &mut budget())?;
    transition.completeness = ProducerCompleteness::CompleteAffectedDomain {
        certificate: certificate_ref,
    };
    transition.events.push(SourceEvent {
        producer: RUNTIME_PRODUCER,
        stream: StreamId([16; 16]),
        stream_epoch: 1,
        predecessor: event_sequence
            .checked_sub(1)
            .ok_or("invalid event sequence")?,
        sequence: event_sequence,
        payload: transition.payload_digest()?,
    });
    Ok(UpdateRequest {
        operation,
        expected_view: view,
        kind: UpdateKind::ProjectionAdvance(Box::new(transition)),
    })
}

fn projection_binding(objects: &FileObjectStore, root: MapRoot) -> TestResult<ProjectionBinding> {
    let mut key = b"projection\0".to_vec();
    key.extend_from_slice(&PROJECTION.0);
    let reference = objects
        .lookup(root, &key, &mut budget())?
        .ok_or("projection missing")?;
    let stored = objects.get(reference, &mut budget())?;
    Ok(serde_json::from_slice(&stored.payload)?)
}

fn coverage(objects: &FileObjectStore, root: MapRoot) -> TestResult<CoverageBinding> {
    let mut key = b"member\0".to_vec();
    key.extend_from_slice(&INSTANCE.0);
    let reference = objects
        .lookup(root, &key, &mut budget())?
        .ok_or("coverage missing")?;
    Ok(serde_json::from_slice(
        &objects.get(reference, &mut budget())?.payload,
    )?)
}

fn owner_coverage(objects: &FileObjectStore, root: MapRoot) -> TestResult<OwnerCoverage> {
    let key = quanta_index_core::index::owner_map_key(owner(RUNTIME_PRODUCER, 2));
    let reference = objects
        .lookup(root, &key, &mut budget())?
        .ok_or("owner coverage missing")?;
    Ok(serde_json::from_slice(
        &objects.get(reference, &mut budget())?.payload,
    )?)
}

#[test]
fn projection_event_sequence_is_independent_of_operation_journal_sequence() -> TestResult {
    let directory = tempfile::tempdir()?;
    let (kernel, objects, catalog) = setup(directory.path())?;
    let genesis = kernel.create_workspace(workspace(), 1, &mut budget())?;
    let profile = stage_profile(&kernel, runtime_operation(1), vec![spec()])?;
    let configured = published(block_on(kernel.submit(
        UpdateRequest {
            operation: runtime_operation(1),
            expected_view: genesis,
            kind: UpdateKind::Reconfigure { profile },
        },
        &mut budget(),
    ))?)?;
    let mut request = projection_request(
        &kernel,
        &catalog,
        &objects,
        configured,
        current(&kernel)?.source,
        None,
        2,
        1,
        SchemaId([23; 16]),
        Digest([24; 32]),
        Vec::new(),
    )?;
    request.operation = runtime_operation(2);
    let published_view = published(block_on(kernel.submit(request.clone(), &mut budget()))?)?;
    assert_eq!(
        kernel.status(request.operation)?,
        OperationStatus::Published(published_view)
    );
    assert_eq!(
        block_on(kernel.submit(request, &mut budget()))?,
        SubmitOutcome::Terminal(OperationStatus::Published(published_view))
    );
    Ok(())
}

#[test]
fn unchanged_source_owner_can_be_replaced_by_producer_certified_dependency_delta() -> TestResult {
    let directory = tempfile::tempdir()?;
    let (kernel, objects, catalog) = setup(directory.path())?;
    let unknown_key = source_operation(99);
    assert_eq!(
        kernel.resume_dispatch(unknown_key, &mut budget())?,
        ResumeDispatch::Reply(SubmitOutcome::Unknown(unknown_key))
    );
    let genesis = kernel.create_workspace(workspace(), 1, &mut budget())?;
    let profile = stage_profile(&kernel, source_operation(1), vec![spec()])?;
    let configure = UpdateRequest {
        operation: source_operation(1),
        expected_view: genesis,
        kind: UpdateKind::Reconfigure { profile },
    };
    assert_eq!(
        OperationExecutionClass::for_request(&configure),
        OperationExecutionClass::Core
    );
    let ingress = kernel.begin_input(configure.operation)?;
    let configure_object =
        objects.put(&serde_json::to_vec(&configure)?, &[profile], &mut budget())?;
    catalog.reserve_inputs(ingress, std::slice::from_ref(&configure_object))?;
    catalog.confirm_inputs(ingress, std::slice::from_ref(&configure_object))?;
    catalog.seal_inputs(ingress, configure_object.reference)?;
    assert!(matches!(
        catalog.admit(
            ImmutableOperation {
                key: configure.operation,
                request: configure_object.reference,
            },
            ingress,
            &[],
            &mut budget(),
        )?,
        AdmissionOutcome::New(_)
    ));
    let pending_core = catalog.inspect(configure.operation)?;
    assert!(matches!(pending_core, OperationStatus::Pending(_)));
    assert_eq!(
        kernel.resume_dispatch(configure.operation, &mut budget())?,
        ResumeDispatch::Execute(OperationExecutionClass::Core)
    );
    assert_eq!(catalog.inspect(configure.operation)?, pending_core);
    let configured = published(block_on(kernel.resume(configure.operation, &mut budget()))?)?;
    let source_a = stage_source(&objects, "a.rs", b"A1")?;
    let source_b = stage_source(&objects, "b.rs", b"B-stable")?;
    let initial = published(block_on(kernel.submit(
        source_request(
            &kernel,
            &catalog,
            &objects,
            configured,
            current(&kernel)?.source,
            2,
            1,
            vec![
                OwnerMutation {
                    owner: owner(SOURCE_PRODUCER, 1),
                    expected: None,
                    next: Some(source_a),
                },
                OwnerMutation {
                    owner: owner(SOURCE_PRODUCER, 2),
                    expected: None,
                    next: Some(source_b),
                },
            ],
        )?,
        &mut budget(),
    ))?)?;
    assert!(matches!(
        coverage(&objects, current(&kernel)?.coverage)?.status,
        CoverageStatus::Pending { .. }
    ));
    let contribution1 = stage_contribution(&objects, Some(source_b), 1)?;
    let first_request = projection_request(
        &kernel,
        &catalog,
        &objects,
        initial,
        current(&kernel)?.source,
        None,
        1,
        1,
        SchemaId([23; 16]),
        Digest([24; 32]),
        vec![OwnerMutation {
            owner: owner(RUNTIME_PRODUCER, 2),
            expected: None,
            next: Some(contribution1),
        }],
    )?;
    let first_projection = published(block_on(kernel.submit(first_request, &mut budget()))?)?;
    let first_binding = projection_binding(&objects, current(&kernel)?.projections)?;
    assert!(matches!(
        coverage(&objects, current(&kernel)?.coverage)?.status,
        CoverageStatus::Partial { .. }
    ));
    let work = kernel.ready_work(workspace(), 1, 10, &mut budget())?;
    let intent = work.into_iter().next().ok_or("owner work missing")?;
    let claim = kernel.claim_work(&intent, Duration::from_secs(120))?;
    let materialize = UpdateRequest {
        operation: source_operation(3),
        expected_view: first_projection,
        kind: UpdateKind::Materialize { claim },
    };
    assert_eq!(
        OperationExecutionClass::for_request(&materialize),
        OperationExecutionClass::Enrichment
    );
    let mut children = Vec::new();
    if let quanta_index_core::index::WorkScope::Owner { expected, .. } = claim.intent.scope {
        children.extend(expected.source);
        children.extend(expected.projection);
    } else {
        return Err("expected owner-scoped work".into());
    }
    children.sort_unstable();
    children.dedup();
    let child_objects = children
        .iter()
        .map(|reference| {
            objects
                .get(*reference, &mut budget())
                .map(|stored| stored.registration)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let ingress = kernel.begin_input(materialize.operation)?;
    catalog.reserve_inputs(ingress, &child_objects)?;
    catalog.confirm_inputs(ingress, &child_objects)?;
    let materialize_object =
        objects.put(&serde_json::to_vec(&materialize)?, &children, &mut budget())?;
    let materialize_reference = materialize_object.reference;
    catalog.reserve_inputs(ingress, std::slice::from_ref(&materialize_object))?;
    catalog.confirm_inputs(ingress, std::slice::from_ref(&materialize_object))?;
    catalog.seal_inputs(ingress, materialize_reference)?;
    assert!(matches!(
        catalog.admit(
            ImmutableOperation {
                key: materialize.operation,
                request: materialize_reference,
            },
            ingress,
            &[],
            &mut budget(),
        )?,
        AdmissionOutcome::New(_)
    ));
    let pending_enrichment = catalog.inspect(materialize.operation)?;
    assert!(matches!(pending_enrichment, OperationStatus::Pending(_)));
    assert_eq!(
        kernel.resume_dispatch(materialize.operation, &mut budget())?,
        ResumeDispatch::Execute(OperationExecutionClass::Enrichment)
    );
    assert_eq!(catalog.inspect(materialize.operation)?, pending_enrichment);
    let request_budget =
        quanta_index_core::RequestBudgetV1::until(Instant::now() + Duration::from_secs(30));
    request_budget.cancel_handle().cancel();
    let mut cancelled = WorkBudget::with_request(
        WorkLimits {
            reads: 1_000_000,
            writes: 1_000_000,
            bytes: 128 * 1024 * 1024,
            mutations: 1_000_000,
        },
        request_budget,
    );
    assert_eq!(
        kernel.resume_dispatch(materialize.operation, &mut cancelled),
        Err(IndexError::Cancelled)
    );
    assert_eq!(catalog.inspect(materialize.operation)?, pending_enrichment);
    let materialized = published(block_on(
        kernel.resume(materialize.operation, &mut budget()),
    )?)?;
    assert_eq!(
        coverage(&objects, current(&kernel)?.coverage)?.status,
        CoverageStatus::Complete
    );
    let source_a2 = stage_source(&objects, "a.rs", b"A2")?;
    let advanced = published(block_on(kernel.submit(
        source_request(
            &kernel,
            &catalog,
            &objects,
            materialized,
            current(&kernel)?.source,
            4,
            2,
            vec![OwnerMutation {
                owner: owner(SOURCE_PRODUCER, 1),
                expected: Some(source_a),
                next: Some(source_a2),
            }],
        )?,
        &mut budget(),
    ))?)?;
    assert!(matches!(
        coverage(&objects, current(&kernel)?.coverage)?.status,
        CoverageStatus::Pending { .. }
    ));
    let contribution2 = stage_contribution(&objects, Some(source_b), 2)?;
    let second_request = projection_request(
        &kernel,
        &catalog,
        &objects,
        advanced,
        current(&kernel)?.source,
        Some(first_binding),
        2,
        2,
        SchemaId([23; 16]),
        Digest([24; 32]),
        vec![OwnerMutation {
            owner: owner(RUNTIME_PRODUCER, 2),
            expected: Some(contribution1),
            next: Some(contribution2),
        }],
    )?;
    let second = published(block_on(
        kernel.submit(second_request.clone(), &mut budget()),
    )?)?;
    let new_binding = projection_binding(&objects, current(&kernel)?.projections)?;
    assert_ne!(new_binding.owners, first_binding.owners);
    assert_eq!(new_binding.source, current(&kernel)?.source);
    let certificate_object = objects.get(new_binding.certificate, &mut budget())?;
    assert!(
        !certificate_object
            .registration
            .children
            .contains(&first_binding.certificate)
    );
    if let Some(prior_root) = first_binding.owners.0 {
        assert!(
            !certificate_object
                .registration
                .children
                .contains(&prior_root)
        );
    }
    assert_eq!(
        projection_binding(
            &objects,
            kernel
                .pin_view(ViewSelection::Exact(first_projection))?
                .view()
                .projections
        )?,
        first_binding
    );
    assert!(matches!(
        coverage(&objects, current(&kernel)?.coverage)?.status,
        CoverageStatus::Partial { .. }
    ));
    assert!(
        matches!(block_on(kernel.submit(second_request, &mut budget()))?, SubmitOutcome::Terminal(OperationStatus::Published(view)) if view == second)
    );
    let stale = projection_request(
        &kernel,
        &catalog,
        &objects,
        second,
        current(&kernel)?.source,
        Some(first_binding),
        3,
        3,
        SchemaId([23; 16]),
        Digest([24; 32]),
        vec![OwnerMutation {
            owner: owner(RUNTIME_PRODUCER, 2),
            expected: Some(contribution1),
            next: Some(contribution1),
        }],
    )?;
    assert!(matches!(
        block_on(kernel.submit(stale, &mut budget()))?,
        SubmitOutcome::Terminal(OperationStatus::Refused(
            quanta_index_core::index::Refusal::Conflict
        ))
    ));
    assert_eq!(current(&kernel)?.identity, second);
    let request_ref = catalog.operation(materialize.operation)?.request;
    let mut name = String::with_capacity(64);
    for byte in (request_ref.0).0 {
        write!(&mut name, "{byte:02x}")?;
    }
    let shard = format!("{:02x}", (request_ref.0).0.first().ok_or("digest empty")?);
    std::fs::remove_file(
        directory
            .path()
            .join("objects/objects")
            .join(shard)
            .join(name),
    )?;
    assert_eq!(
        kernel.resume_dispatch(materialize.operation, &mut budget())?,
        ResumeDispatch::Reply(SubmitOutcome::Terminal(OperationStatus::Published(
            materialized
        )))
    );
    assert_eq!(
        block_on(kernel.resume(materialize.operation, &mut budget()))?,
        SubmitOutcome::Terminal(OperationStatus::Published(materialized))
    );
    assert_eq!(current(&kernel)?.identity, second);
    Ok(())
}

#[test]
fn partial_and_wrong_producer_do_not_advance_head() -> TestResult {
    let directory = tempfile::tempdir()?;
    let (kernel, objects, catalog) = setup(directory.path())?;
    let genesis = kernel.create_workspace(workspace(), 1, &mut budget())?;
    let profile = stage_profile(&kernel, source_operation(1), vec![spec()])?;
    let configured = published(block_on(kernel.submit(
        UpdateRequest {
            operation: source_operation(1),
            expected_view: genesis,
            kind: UpdateKind::Reconfigure { profile },
        },
        &mut budget(),
    ))?)?;
    let source = current(&kernel)?.source;
    let mut partial = ProjectionTransition {
        projection: PROJECTION,
        producer: RUNTIME_PRODUCER,
        expected: None,
        target_source: source,
        schema: SchemaId([23; 16]),
        producer_policy: Digest([24; 32]),
        dependency_context: Digest([25; 32]),
        completeness: ProducerCompleteness::Partial {
            missing: MapRoot(None),
        },
        owners: stage_manifest(
            &kernel,
            &catalog,
            &objects,
            runtime_operation(1),
            Vec::new(),
        )?,
        events: Vec::new(),
    };
    partial.events.push(SourceEvent {
        producer: RUNTIME_PRODUCER,
        stream: StreamId([16; 16]),
        stream_epoch: 1,
        predecessor: 0,
        sequence: 1,
        payload: partial.payload_digest()?,
    });
    let outcome = block_on(kernel.submit(
        UpdateRequest {
            operation: runtime_operation(1),
            expected_view: configured,
            kind: UpdateKind::ProjectionAdvance(Box::new(partial.clone())),
        },
        &mut budget(),
    ))?;
    assert!(matches!(
        outcome,
        SubmitOutcome::Terminal(OperationStatus::Refused(
            quanta_index_core::index::Refusal::RebuildRequired
        ))
    ));
    assert_eq!(current(&kernel)?.identity, configured);
    let mut wrong = partial;
    wrong.producer = SOURCE_PRODUCER;
    let outcome = block_on(kernel.submit(
        UpdateRequest {
            operation: runtime_operation(2),
            expected_view: configured,
            kind: UpdateKind::ProjectionAdvance(Box::new(wrong)),
        },
        &mut budget(),
    ));
    assert!(matches!(outcome, Err(IndexError::InvalidInput(_))));
    assert_eq!(current(&kernel)?.identity, configured);
    Ok(())
}

#[test]
fn schema_or_policy_change_cannot_rebind_unchanged_projection_outputs() -> TestResult {
    for (schema, policy) in [
        (SchemaId([26; 16]), Digest([24; 32])),
        (SchemaId([23; 16]), Digest([27; 32])),
    ] {
        let directory = tempfile::tempdir()?;
        let (kernel, objects, catalog) = setup(directory.path())?;
        let genesis = kernel.create_workspace(workspace(), 1, &mut budget())?;
        let profile = stage_profile(&kernel, source_operation(1), vec![spec()])?;
        let configured = published(block_on(kernel.submit(
            UpdateRequest {
                operation: source_operation(1),
                expected_view: genesis,
                kind: UpdateKind::Reconfigure { profile },
            },
            &mut budget(),
        ))?)?;
        let first = projection_request(
            &kernel,
            &catalog,
            &objects,
            configured,
            current(&kernel)?.source,
            None,
            1,
            1,
            SchemaId([23; 16]),
            Digest([24; 32]),
            Vec::new(),
        )?;
        let first_view = published(block_on(kernel.submit(first, &mut budget()))?)?;
        let prior = projection_binding(&objects, current(&kernel)?.projections)?;
        let altered = projection_request(
            &kernel,
            &catalog,
            &objects,
            first_view,
            current(&kernel)?.source,
            Some(prior),
            2,
            2,
            schema,
            policy,
            Vec::new(),
        )?;
        assert!(matches!(
            block_on(kernel.submit(altered, &mut budget()))?,
            SubmitOutcome::Terminal(OperationStatus::Refused(
                quanta_index_core::index::Refusal::RebuildRequired
            ))
        ));
        assert_eq!(current(&kernel)?.identity, first_view);
    }
    Ok(())
}

#[test]
fn reconfigure_cannot_inherit_old_projection_owner_under_new_recipe() -> TestResult {
    let directory = tempfile::tempdir()?;
    let (kernel, objects, catalog) = setup(directory.path())?;
    let genesis = kernel.create_workspace(workspace(), 1, &mut budget())?;
    let profile = stage_profile(&kernel, source_operation(1), vec![spec()])?;
    let configured = published(block_on(kernel.submit(
        UpdateRequest {
            operation: source_operation(1),
            expected_view: genesis,
            kind: UpdateKind::Reconfigure { profile },
        },
        &mut budget(),
    ))?)?;
    let contribution = stage_contribution(&objects, None, 1)?;
    let first = projection_request(
        &kernel,
        &catalog,
        &objects,
        configured,
        current(&kernel)?.source,
        None,
        1,
        1,
        SchemaId([23; 16]),
        Digest([24; 32]),
        vec![OwnerMutation {
            owner: owner(RUNTIME_PRODUCER, 2),
            expected: None,
            next: Some(contribution),
        }],
    )?;
    let projected = published(block_on(kernel.submit(first, &mut budget()))?)?;
    let mut changed_policy = spec();
    changed_policy.policy = Digest([31; 32]);
    let mut changed_schema = spec();
    changed_schema.kind = IndexKind([28; 16]);
    changed_schema.schema = SchemaId([29; 16]);
    changed_schema.format = StorageFormatId([30; 16]);
    for (sequence, next_spec) in [(2, changed_policy), (3, changed_schema)] {
        let target = stage_profile(&kernel, source_operation(sequence), vec![next_spec])?;
        assert!(matches!(
            block_on(kernel.submit(
                UpdateRequest {
                    operation: source_operation(sequence),
                    expected_view: projected,
                    kind: UpdateKind::Reconfigure { profile: target },
                },
                &mut budget(),
            ))?,
            SubmitOutcome::Terminal(OperationStatus::Refused(
                quanta_index_core::index::Refusal::RebuildRequired
            ))
        ));
        assert_eq!(current(&kernel)?.identity, projected);
    }
    Ok(())
}

#[test]
fn repeated_source_advances_preserve_pending_missing_until_certified_projection_advance()
-> TestResult {
    let directory = tempfile::tempdir()?;
    let (kernel, objects, catalog) = setup(directory.path())?;
    let genesis = kernel.create_workspace(workspace(), 1, &mut budget())?;
    let profile = stage_profile(&kernel, source_operation(1), vec![spec()])?;
    let configured = published(block_on(kernel.submit(
        UpdateRequest {
            operation: source_operation(1),
            expected_view: genesis,
            kind: UpdateKind::Reconfigure { profile },
        },
        &mut budget(),
    ))?)?;
    let source_a0 = stage_source(&objects, "a.rs", b"A0")?;
    let source_b = stage_source(&objects, "b.rs", b"B")?;
    let initial = published(block_on(kernel.submit(
        source_request(
            &kernel,
            &catalog,
            &objects,
            configured,
            current(&kernel)?.source,
            2,
            1,
            vec![
                OwnerMutation {
                    owner: owner(SOURCE_PRODUCER, 1),
                    expected: None,
                    next: Some(source_a0),
                },
                OwnerMutation {
                    owner: owner(SOURCE_PRODUCER, 2),
                    expected: None,
                    next: Some(source_b),
                },
            ],
        )?,
        &mut budget(),
    ))?)?;
    let contribution = stage_contribution(&objects, Some(source_b), 1)?;
    let first = published(block_on(kernel.submit(
        projection_request(
            &kernel,
            &catalog,
            &objects,
            initial,
            current(&kernel)?.source,
            None,
            1,
            1,
            SchemaId([23; 16]),
            Digest([24; 32]),
            vec![OwnerMutation {
                owner: owner(RUNTIME_PRODUCER, 2),
                expected: None,
                next: Some(contribution),
            }],
        )?,
        &mut budget(),
    ))?)?;
    let prior_binding = projection_binding(&objects, current(&kernel)?.projections)?;
    let before = coverage(&objects, current(&kernel)?.coverage)?;
    let CoverageStatus::Partial {
        missing: original_missing,
    } = before.status
    else {
        return Err("initial owner work must be missing".into());
    };
    assert!(original_missing.0.is_some());
    let source_a1 = stage_source(&objects, "a.rs", b"A1")?;
    let advanced_once = published(block_on(kernel.submit(
        source_request(
            &kernel,
            &catalog,
            &objects,
            first,
            current(&kernel)?.source,
            3,
            2,
            vec![OwnerMutation {
                owner: owner(SOURCE_PRODUCER, 1),
                expected: Some(source_a0),
                next: Some(source_a1),
            }],
        )?,
        &mut budget(),
    ))?)?;
    assert!(
        matches!(coverage(&objects, current(&kernel)?.coverage)?.status,
        CoverageStatus::Pending { previous_missing } if previous_missing == original_missing)
    );
    let source_a2 = stage_source(&objects, "a.rs", b"A2")?;
    let advanced_twice = published(block_on(kernel.submit(
        source_request(
            &kernel,
            &catalog,
            &objects,
            advanced_once,
            current(&kernel)?.source,
            4,
            3,
            vec![OwnerMutation {
                owner: owner(SOURCE_PRODUCER, 1),
                expected: Some(source_a1),
                next: Some(source_a2),
            }],
        )?,
        &mut budget(),
    ))?)?;
    assert!(
        matches!(coverage(&objects, current(&kernel)?.coverage)?.status,
        CoverageStatus::Pending { previous_missing } if previous_missing == original_missing)
    );
    let _certified = published(block_on(kernel.submit(
        projection_request(
            &kernel,
            &catalog,
            &objects,
            advanced_twice,
            current(&kernel)?.source,
            Some(prior_binding),
            2,
            2,
            SchemaId([23; 16]),
            Digest([24; 32]),
            Vec::new(),
        )?,
        &mut budget(),
    ))?)?;
    let after = coverage(&objects, current(&kernel)?.coverage)?;
    assert_eq!(
        after.status,
        CoverageStatus::Partial {
            missing: original_missing
        }
    );
    assert_eq!(after.covered, None);
    let owner = owner_coverage(&objects, after.owners)?;
    assert_eq!(owner.covered, None);
    assert_eq!(owner.searchable, None);
    assert_eq!(
        kernel.ready_work(workspace(), 1, 10, &mut budget())?.len(),
        1
    );
    Ok(())
}

#[test]
fn certified_empty_contribution_clears_current_searchable_artifact() -> TestResult {
    let directory = tempfile::tempdir()?;
    let (kernel, objects, catalog) = setup(directory.path())?;
    let genesis = kernel.create_workspace(workspace(), 1, &mut budget())?;
    let profile = stage_profile(&kernel, source_operation(1), vec![spec()])?;
    let configured = published(block_on(kernel.submit(
        UpdateRequest {
            operation: source_operation(1),
            expected_view: genesis,
            kind: UpdateKind::Reconfigure { profile },
        },
        &mut budget(),
    ))?)?;
    let present = stage_contribution(&objects, None, 1)?;
    let first = published(block_on(kernel.submit(
        projection_request(
            &kernel,
            &catalog,
            &objects,
            configured,
            current(&kernel)?.source,
            None,
            1,
            1,
            SchemaId([23; 16]),
            Digest([24; 32]),
            vec![OwnerMutation {
                owner: owner(RUNTIME_PRODUCER, 2),
                expected: None,
                next: Some(present),
            }],
        )?,
        &mut budget(),
    ))?)?;
    let prior_binding = projection_binding(&objects, current(&kernel)?.projections)?;
    let intent = kernel
        .ready_work(workspace(), 1, 10, &mut budget())?
        .into_iter()
        .next()
        .ok_or("owner work missing")?;
    let claim = kernel.claim_work(&intent, Duration::from_secs(10))?;
    let materialized = published(block_on(kernel.submit(
        UpdateRequest {
            operation: source_operation(2),
            expected_view: first,
            kind: UpdateKind::Materialize { claim },
        },
        &mut budget(),
    ))?)?;
    let old_view = kernel
        .pin_view(ViewSelection::Exact(materialized))?
        .view()
        .clone();
    let old = owner_coverage(&objects, coverage(&objects, old_view.coverage)?.owners)?;
    assert!(old.searchable.is_some());
    let empty = stage_empty_contribution(&objects, 2)?;
    let _replaced = published(block_on(kernel.submit(
        projection_request(
            &kernel,
            &catalog,
            &objects,
            materialized,
            current(&kernel)?.source,
            Some(prior_binding),
            2,
            2,
            SchemaId([23; 16]),
            Digest([24; 32]),
            vec![OwnerMutation {
                owner: owner(RUNTIME_PRODUCER, 2),
                expected: Some(present),
                next: Some(empty),
            }],
        )?,
        &mut budget(),
    ))?)?;
    let now = coverage(&objects, current(&kernel)?.coverage)?;
    let current_owner = owner_coverage(&objects, now.owners)?;
    assert_eq!(now.status, CoverageStatus::Complete);
    assert_eq!(current_owner.covered, Some(current_owner.desired));
    assert_eq!(current_owner.desired.projection, Some(empty));
    assert_eq!(current_owner.searchable, None);
    assert_ne!(current_owner.searchable, old.searchable);
    assert!(
        kernel
            .ready_work(workspace(), 1, 10, &mut budget())?
            .is_empty()
    );
    assert_eq!(
        owner_coverage(&objects, coverage(&objects, old_view.coverage)?.owners)?.searchable,
        old.searchable
    );
    Ok(())
}
