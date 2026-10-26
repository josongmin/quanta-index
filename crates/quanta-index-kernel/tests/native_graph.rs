//! Certified graph contributions reach a selected core member and typed query.
use std::{
    collections::BTreeSet,
    error::Error,
    fmt::Write as _,
    future::Future,
    path::Path,
    sync::Arc,
    task::{Context, Poll, Wake, Waker},
    time::Duration,
};

macro_rules! check {
    ($condition:expr) => {
        if !$condition {
            return Err(format!("graph fixture check failed: {}", stringify!($condition)).into());
        }
    };
}

macro_rules! check_eq {
    ($left:expr, $right:expr) => {
        match (&$left, &$right) {
            (left, right) if left != right => {
                return Err(format!(
                    "graph fixture equality failed: {} == {}; left={left:?}, right={right:?}",
                    stringify!($left),
                    stringify!($right)
                )
                .into());
            }
            _ => {}
        }
    };
}

struct TestWake(std::thread::Thread);

impl Wake for TestWake {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.0.unpark();
    }
}

fn run<F: Future>(future: F) -> F::Output {
    let waker = Waker::from(Arc::new(TestWake(std::thread::current())));
    let mut context = Context::from_waker(&waker);
    let mut future = std::pin::pin!(future);
    loop {
        match future.as_mut().poll(&mut context) {
            Poll::Ready(output) => return output,
            Poll::Pending => std::thread::park(),
        }
    }
}
use quanta_index_catalog::SqliteCatalog;
use quanta_index_contract::{
    FileId, RepoMapEdgeKind, RepoMapNodeRef,
    native_protocol::{GraphEdge, GraphNode, GraphOwnerOutput, NativeGraphDirection},
};
use quanta_index_core::index::{
    CollectionId, Digest, GroupActivation, IndexError, IndexInstanceId, IndexKind, IndexProfile,
    IngressRef, InputDomain, InstanceSpec, MapRoot, ObjectRef, OperationKey, OperationStatus,
    OwnerKey, OwnerMutation, ProducerCompleteness, ProducerId, ProjectionBinding,
    ProjectionCertificate, ProjectionId, ProjectionOwnerContribution, ProjectionTransition,
    PublicationGroupId, SchemaId, SlotId, SourceEvent, SourceTextAdmission, SourceTransition,
    StableOwnerId, StorageFormatId, StreamId, SubmitOutcome, TenantId, UpdateKind, UpdateRequest,
    ViewRef, ViewSelection, WorkBudget, WorkLimits, WorkspaceKey, WorktreeId,
    effort::{ReservationGuard, ResourceNeed, ResourceScheduler},
    ports::{ImmutableObjectPort, InputStagingPort, PersistentMapPort},
};
use quanta_index_kernel::{
    AdapterDescriptor, AdapterRegistry, Kernel, KernelServices, QueryAdapterRegistry,
};
use quanta_index_repomap::native::{GraphPage, GraphQuery, NativeGraph, NativeGraphReader};
use quanta_index_storage::{FileObjectStore, FileScratchSpace};

type TestResult<T = ()> = Result<T, Box<dyn Error>>;
const PRODUCER: ProducerId = ProducerId([41; 16]);
const PROJECTION: ProjectionId = ProjectionId([42; 16]);
const INSTANCE: IndexInstanceId = IndexInstanceId([43; 16]);
const SCHEMA: SchemaId = SchemaId([44; 16]);
const POLICY: Digest = Digest([45; 32]);
const SOURCE_STREAM: StreamId = StreamId([47; 16]);

fn budget() -> WorkBudget {
    WorkBudget::with_request(
        WorkLimits {
            reads: 1_000_000,
            writes: 1_000_000,
            bytes: 128 * 1024 * 1024,
            mutations: 1_000_000,
        },
        quanta_index_core::RequestBudgetV1::for_duration(Duration::from_secs(30)),
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

fn operation(sequence: u64) -> OperationKey {
    OperationKey {
        workspace: workspace(),
        epoch: 1,
        producer: PRODUCER,
        stream: StreamId([46; 16]),
        stream_epoch: 1,
        sequence,
    }
}

fn owner(id: u8) -> OwnerKey {
    OwnerKey {
        producer: PRODUCER,
        stable_id: StableOwnerId([id; 16]),
    }
}

fn spec() -> InstanceSpec {
    InstanceSpec {
        instance: INSTANCE,
        kind: IndexKind([47; 16]),
        group: PublicationGroupId([48; 16]),
        activation: GroupActivation::CoreRequired,
        input_domain: InputDomain::Projection {
            projection: PROJECTION,
            producer: PRODUCER,
        },
        schema: SCHEMA,
        policy: POLICY,
        format: StorageFormatId([49; 16]),
    }
}

fn descriptor() -> AdapterDescriptor {
    let instance = spec();
    AdapterDescriptor {
        kind: instance.kind,
        schema: instance.schema,
        format: instance.format,
    }
}

fn need() -> ResourceNeed {
    ResourceNeed {
        cpu_units: 1,
        io_bytes: 1024 * 1024,
        memory_bytes: 4 * 1024 * 1024,
        provider_units: 0,
    }
}

struct TestScheduler;
impl ResourceScheduler for TestScheduler {
    type Token = ();
    fn try_reserve(&self, _need: ResourceNeed) -> Option<Self::Token> {
        Some(())
    }
    fn release(&self, _token: Self::Token) {}
}

fn guard() -> TestResult<ReservationGuard<TestScheduler>> {
    ReservationGuard::try_acquire(Arc::new(TestScheduler), need())
        .ok_or_else(|| "graph query reservation refused".into())
}

struct Fixture {
    kernel: Kernel<SqliteCatalog>,
    objects: Arc<FileObjectStore>,
    catalog: Arc<SqliteCatalog>,
    readers: QueryAdapterRegistry<GraphQuery, GraphPage>,
}

fn fixture(root: &Path) -> TestResult<Fixture> {
    let objects = Arc::new(FileObjectStore::open(root.join("objects"))?);
    let catalog = Arc::new(SqliteCatalog::open(root, Duration::from_millis(100))?);
    let mut registry = AdapterRegistry::new();
    registry.register_projection(descriptor(), NativeGraph)?;
    let kernel = Kernel::new(
        KernelServices::new(
            objects.clone(),
            objects.clone(),
            catalog.clone(),
            Arc::new(FileScratchSpace::open_standalone(
                root.join("scratch"),
                16 * 1024 * 1024 * 1024,
            )?),
            Duration::from_secs(30),
        )?,
        registry,
    );
    let mut readers = QueryAdapterRegistry::new();
    readers.register(descriptor(), need(), NativeGraphReader::new(need())?)?;
    Ok(Fixture {
        kernel,
        objects,
        catalog,
        readers,
    })
}

fn published(outcome: SubmitOutcome) -> TestResult<ViewRef> {
    match outcome {
        SubmitOutcome::Terminal(OperationStatus::Published(view)) => Ok(view),
        SubmitOutcome::Terminal(
            status @ (OperationStatus::Unknown
            | OperationStatus::Pending(_)
            | OperationStatus::Refused(_)),
        ) => Err(format!("expected graph publication, got terminal {status:?}").into()),
        other @ (SubmitOutcome::Accepted(_) | SubmitOutcome::Unknown(_)) => {
            Err(format!("expected graph publication: {other:?}").into())
        }
    }
}

fn node(path: &str) -> RepoMapNodeRef {
    RepoMapNodeRef::File(FileId::new(path))
}

fn output() -> GraphOwnerOutput {
    GraphOwnerOutput {
        projection: Vec::new(),
        nodes: vec![
            GraphNode {
                identity: node("src/a.rs"),
                payload_digest: Digest([11; 32]),
            },
            GraphNode {
                identity: node("src/b.rs"),
                payload_digest: Digest([12; 32]),
            },
        ],
        edges: vec![GraphEdge {
            kind: RepoMapEdgeKind::DependsOn,
            source: node("src/a.rs"),
            target: node("src/b.rs"),
        }],
    }
}

fn contribution(objects: &FileObjectStore, id: u8) -> TestResult<ObjectRef> {
    contribution_with_output(objects, id, &output())
}

fn contribution_with_output(
    objects: &FileObjectStore,
    id: u8,
    graph: &GraphOwnerOutput,
) -> TestResult<ObjectRef> {
    let output = objects
        .put(&serde_json::to_vec(graph)?, &[], &mut budget())?
        .reference;
    let value = ProjectionOwnerContribution {
        projection: PROJECTION,
        producer: PRODUCER,
        owner: owner(id),
        source_owner: None,
        source: None,
        dependency_stamp: Digest([id; 32]),
        output: Some(output),
    };
    Ok(objects
        .put(
            &serde_json::to_vec(&value)?,
            &value.object_children(),
            &mut budget(),
        )?
        .reference)
}

#[test]
fn staged_projection_output_retains_bounded_canonical_children() -> TestResult {
    let directory = tempfile::tempdir()?;
    let fixture = fixture(directory.path())?;
    let _genesis = fixture
        .kernel
        .create_workspace(workspace(), 1, &mut budget())?;
    let ingress = fixture.kernel.begin_input(operation(101))?;
    let first = fixture
        .kernel
        .stage_projection_output(ingress, b"rank row one", &mut budget())?;
    let second = fixture
        .kernel
        .stage_projection_output(ingress, b"rank row two", &mut budget())?;
    check!(
        fixture
            .objects
            .get(first, &mut budget())?
            .registration
            .children
            .is_empty()
    );
    let mut children = [first, second];
    children.sort_unstable();
    let output = fixture.kernel.stage_projection_output_with_children(
        ingress,
        b"rank output",
        &children,
        &mut budget(),
    )?;
    check_eq!(
        fixture
            .objects
            .get(output, &mut budget())?
            .registration
            .children,
        children.to_vec()
    );
    let contribution = ProjectionOwnerContribution {
        projection: PROJECTION,
        producer: PRODUCER,
        owner: owner(1),
        source_owner: None,
        source: None,
        dependency_stamp: Digest([101; 32]),
        output: Some(output),
    };
    let staged_owner =
        fixture
            .kernel
            .stage_projection_owner(ingress, contribution, &mut budget())?;
    check_eq!(
        fixture
            .objects
            .get(staged_owner, &mut budget())?
            .registration
            .children,
        vec![output]
    );
    // Kernel preserves a bounded immutable DAG. The graph adapter checks
    // whether an output actually names canonical leaf rows.
    let nested = fixture.kernel.stage_projection_output_with_children(
        ingress,
        b"nested output",
        &[output],
        &mut budget(),
    )?;
    let nested_owner = fixture.kernel.stage_projection_owner(
        ingress,
        ProjectionOwnerContribution {
            output: Some(nested),
            ..contribution
        },
        &mut budget(),
    )?;
    check_eq!(
        fixture
            .objects
            .get(nested_owner, &mut budget())?
            .registration
            .children,
        vec![nested]
    );

    let reversed = [children[1], children[0]];
    check!(matches!(
        fixture.kernel.stage_projection_output_with_children(
            ingress,
            b"wrong order",
            &reversed,
            &mut budget()
        ),
        Err(IndexError::InvalidInput(_))
    ));
    check!(matches!(
        fixture.kernel.stage_projection_output_with_children(
            ingress,
            b"duplicate",
            &[first, first],
            &mut budget()
        ),
        Err(IndexError::InvalidInput(_))
    ));
    let overbound = vec![first; quanta_index_core::index::MAX_RECLAIM_CHILDREN + 1];
    check!(matches!(
        fixture.kernel.stage_projection_output_with_children(
            ingress,
            b"overbound",
            &overbound,
            &mut budget()
        ),
        Err(IndexError::ResourceLimited)
    ));
    let missing = ObjectRef(Digest([255; 32]));
    check!(matches!(
        fixture.kernel.stage_projection_output_with_children(
            ingress,
            b"missing child",
            &[missing],
            &mut budget()
        ),
        Err(IndexError::InvalidInput(_) | IndexError::CorruptObject(_))
    ));
    Ok(())
}

fn adopt(
    fixture: &Fixture,
    ingress: IngressRef,
    reference: ObjectRef,
    seen: &mut BTreeSet<ObjectRef>,
) -> TestResult {
    if !seen.insert(reference) {
        return Ok(());
    }
    let object = fixture.objects.get(reference, &mut budget())?;
    for child in object.registration.children.iter().copied() {
        adopt(fixture, ingress, child, seen)?;
    }
    fixture
        .catalog
        .reserve_inputs(ingress, std::slice::from_ref(&object.registration))?;
    fixture
        .catalog
        .confirm_inputs(ingress, std::slice::from_ref(&object.registration))?;
    Ok(())
}

fn current(fixture: &Fixture) -> TestResult<quanta_index_core::index::CatalogView> {
    Ok(fixture
        .kernel
        .pin_view(
            ViewSelection::Latest {
                workspace: workspace(),
                epoch: 1,
            },
            &budget(),
        )?
        .view()
        .clone())
}

fn projection_binding(fixture: &Fixture, root: MapRoot) -> TestResult<ProjectionBinding> {
    let mut key = b"projection\0".to_vec();
    key.extend_from_slice(&PROJECTION.0);
    let reference = fixture
        .objects
        .lookup_object(root, &key, &mut budget())?
        .ok_or("graph projection missing")?;
    Ok(serde_json::from_slice(
        &fixture.objects.get(reference, &mut budget())?.payload,
    )?)
}

fn member_artifact(fixture: &Fixture, view: ViewRef) -> TestResult<ObjectRef> {
    let selected = fixture
        .kernel
        .pin_view(ViewSelection::Exact(view), &budget())?;
    let key = quanta_index_core::index::member_map_key(INSTANCE);
    let reference = fixture
        .objects
        .lookup_object(selected.view().members, &key, &mut budget())?
        .ok_or("graph member missing")?;
    let stored = fixture.objects.get(reference, &mut budget())?;
    let binding: quanta_index_core::index::MemberBinding = serde_json::from_slice(&stored.payload)?;
    Ok(binding.artifact)
}

fn advance(
    fixture: &Fixture,
    view: ViewRef,
    prior: Option<&ProjectionBinding>,
    operation_sequence: u64,
    event_sequence: u64,
    mutations: Vec<OwnerMutation>,
) -> TestResult<ViewRef> {
    published(advance_outcome(
        fixture,
        view,
        prior,
        operation_sequence,
        event_sequence,
        mutations,
    )?)
}

fn advance_outcome(
    fixture: &Fixture,
    view: ViewRef,
    prior: Option<&ProjectionBinding>,
    operation_sequence: u64,
    event_sequence: u64,
    mutations: Vec<OwnerMutation>,
) -> TestResult<SubmitOutcome> {
    let key = operation(operation_sequence);
    let ingress = fixture.kernel.begin_input(key)?;
    let mut seen = BTreeSet::new();
    for reference in mutations.iter().filter_map(|item| item.next) {
        adopt(fixture, ingress, reference, &mut seen)?;
    }
    let affected =
        fixture
            .kernel
            .stage_mutations(ingress, mutations.into_iter().map(Ok), &mut budget())?;
    let mut transition = ProjectionTransition {
        projection: PROJECTION,
        producer: PRODUCER,
        expected: prior.copied(),
        target_source: current(fixture)?.source,
        schema: SCHEMA,
        producer_policy: POLICY,
        dependency_context: Digest([50; 32]),
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
    let certificate_ref =
        fixture
            .kernel
            .stage_projection_certificate(ingress, &certificate, &mut budget())?;
    transition.completeness = ProducerCompleteness::CompleteAffectedDomain {
        certificate: certificate_ref,
    };
    transition.events.push(SourceEvent {
        producer: PRODUCER,
        stream: StreamId([46; 16]),
        stream_epoch: 1,
        predecessor: event_sequence
            .checked_sub(1)
            .ok_or("event sequence has no predecessor")?,
        sequence: event_sequence,
        payload: transition.payload_digest()?,
    });
    Ok(run(fixture.kernel.submit(
        UpdateRequest {
            operation: key,
            expected_view: view,
            kind: UpdateKind::ProjectionAdvance(Box::new(transition)),
        },
        &mut budget(),
    ))?)
}

fn configure(fixture: &Fixture, view: ViewRef, sequence: u64) -> TestResult<ViewRef> {
    let key = operation(sequence);
    let ingress = fixture.kernel.begin_input(key)?;
    let profile = fixture.kernel.stage_profile(
        ingress,
        &IndexProfile {
            instances: vec![spec()],
        },
        &mut budget(),
    )?;
    published(run(fixture.kernel.submit(
        UpdateRequest {
            operation: key,
            expected_view: view,
            kind: UpdateKind::Reconfigure { profile },
        },
        &mut budget(),
    ))?)
}

fn advance_source(fixture: &Fixture, view: ViewRef) -> TestResult<ViewRef> {
    // Projection events advance the durable producer stream but do not enter
    // the source cut's frontier map. A source event starts its own stream.
    let key = OperationKey {
        stream: SOURCE_STREAM,
        ..operation(7)
    };
    let ingress = fixture.kernel.begin_input(key)?;
    let record = fixture.kernel.stage_source_text(
        ingress,
        "src/new.rs",
        b"new source",
        SourceTextAdmission::Excluded,
        &mut budget(),
    )?;
    let owners = fixture.kernel.stage_mutations(
        ingress,
        std::iter::once(Ok(OwnerMutation {
            owner: owner(9),
            expected: None,
            next: Some(record),
        })),
        &mut budget(),
    )?;
    let mut transition = SourceTransition {
        expected_source: current(fixture)?.source,
        owners,
        events: Vec::new(),
    };
    transition.events.push(SourceEvent {
        producer: PRODUCER,
        stream: SOURCE_STREAM,
        stream_epoch: 1,
        predecessor: 0,
        sequence: 1,
        payload: transition.owner_digest()?,
    });
    published(run(fixture.kernel.submit(
        UpdateRequest {
            operation: key,
            expected_view: view,
            kind: UpdateKind::SourceAdvance(transition),
        },
        &mut budget(),
    ))?)
}

fn query(fixture: &Fixture, view: ViewRef) -> TestResult<GraphPage> {
    query_with_bounds(fixture, view, 8, 8)
}

fn query_with_bounds(
    fixture: &Fixture,
    view: ViewRef,
    max_supporters: u32,
    max_edges: u32,
) -> TestResult<GraphPage> {
    query_direction_with_bounds(
        fixture,
        view,
        node("src/a.rs"),
        NativeGraphDirection::Outgoing,
        max_supporters,
        max_edges,
    )
}

fn query_incoming(fixture: &Fixture, view: ViewRef) -> TestResult<GraphPage> {
    query_direction_with_bounds(
        fixture,
        view,
        node("src/b.rs"),
        NativeGraphDirection::Incoming,
        8,
        8,
    )
}

fn query_direction_with_bounds(
    fixture: &Fixture,
    view: ViewRef,
    source: RepoMapNodeRef,
    direction: NativeGraphDirection,
    max_supporters: u32,
    max_edges: u32,
) -> TestResult<GraphPage> {
    let mut work = budget();
    let prepared = fixture.kernel.prepare_query(
        &fixture.readers,
        ViewSelection::Exact(view),
        INSTANCE,
        GraphQuery {
            source,
            direction,
            max_supporters,
            max_edges,
        },
        &mut work,
    )?;
    Ok(prepared.execute(&guard()?)?.into_result())
}

#[test]
fn sparse_delta_matches_independently_bootstrapped_final_graph() -> TestResult {
    let sparse_root = tempfile::tempdir()?;
    let sparse = fixture(sparse_root.path())?;
    let genesis = sparse
        .kernel
        .create_workspace(workspace(), 1, &mut budget())?;
    let first = contribution(&sparse.objects, 1)?;
    let second = contribution(&sparse.objects, 2)?;
    let two = advance(
        &sparse,
        genesis,
        None,
        1,
        1,
        vec![
            OwnerMutation {
                owner: owner(1),
                expected: None,
                next: Some(first),
            },
            OwnerMutation {
                owner: owner(2),
                expected: None,
                next: Some(second),
            },
        ],
    )?;
    let two = configure(&sparse, two, 2)?;
    let prior = projection_binding(&sparse, current(&sparse)?.projections)?;
    let one = advance(
        &sparse,
        two,
        Some(&prior),
        3,
        2,
        vec![OwnerMutation {
            owner: owner(1),
            expected: Some(first),
            next: None,
        }],
    )?;
    let delta_page = query(&sparse, one)?;
    assert_support(&delta_page, 1);
    assert_support(&query(&sparse, two)?, 2);
    let delta_incoming = query_incoming(&sparse, one)?;
    assert_incoming_support(&delta_incoming, 1);
    assert_incoming_support(&query_incoming(&sparse, two)?, 2);

    let full_root = tempfile::tempdir()?;
    let full = fixture(full_root.path())?;
    let genesis = full
        .kernel
        .create_workspace(workspace(), 1, &mut budget())?;
    let only = contribution(&full.objects, 2)?;
    let bootstrapped = advance(
        &full,
        genesis,
        None,
        1,
        1,
        vec![OwnerMutation {
            owner: owner(2),
            expected: None,
            next: Some(only),
        }],
    )?;
    let bootstrapped = configure(&full, bootstrapped, 2)?;
    let full_page = query(&full, bootstrapped)?;
    assert_support(&full_page, 1);
    check_eq!(delta_page, full_page);
    let full_incoming = query_incoming(&full, bootstrapped)?;
    assert_incoming_support(&full_incoming, 1);
    check_eq!(delta_incoming, full_incoming);
    Ok(())
}

#[test]
fn distinct_edges_each_admit_their_own_supporter_bound() -> TestResult {
    let root = tempfile::tempdir()?;
    let fixture = fixture(root.path())?;
    let genesis = fixture
        .kernel
        .create_workspace(workspace(), 1, &mut budget())?;
    let graph = GraphOwnerOutput {
        projection: Vec::new(),
        nodes: vec![
            GraphNode {
                identity: node("src/a.rs"),
                payload_digest: Digest([11; 32]),
            },
            GraphNode {
                identity: node("src/b.rs"),
                payload_digest: Digest([12; 32]),
            },
            GraphNode {
                identity: node("src/c.rs"),
                payload_digest: Digest([13; 32]),
            },
        ],
        edges: vec![
            GraphEdge {
                kind: RepoMapEdgeKind::DependsOn,
                source: node("src/a.rs"),
                target: node("src/b.rs"),
            },
            GraphEdge {
                kind: RepoMapEdgeKind::DependsOn,
                source: node("src/a.rs"),
                target: node("src/c.rs"),
            },
            GraphEdge {
                kind: RepoMapEdgeKind::DependsOn,
                source: node("src/c.rs"),
                target: node("src/b.rs"),
            },
        ],
    };
    let contribution = contribution_with_output(&fixture.objects, 1, &graph)?;
    let view = advance(
        &fixture,
        genesis,
        None,
        1,
        1,
        vec![OwnerMutation {
            owner: owner(1),
            expected: None,
            next: Some(contribution),
        }],
    )?;
    let view = configure(&fixture, view, 2)?;
    let page = query_with_bounds(&fixture, view, 1, 2)?;
    check_eq!(page.source_supporters, 1);
    check_eq!(page.edges.len(), 2);
    let [first, second] = page.edges.as_slice() else {
        return Err("graph page did not contain exactly two edges".into());
    };
    check_eq!(first.edge.target, node("src/b.rs"));
    check_eq!(second.edge.target, node("src/c.rs"));
    check_eq!(first.supporters, 1);
    check_eq!(second.supporters, 1);
    let incoming = query_direction_with_bounds(
        &fixture,
        view,
        node("src/b.rs"),
        NativeGraphDirection::Incoming,
        1,
        2,
    )?;
    check_eq!(incoming.source_supporters, 1);
    check_eq!(
        incoming.source.as_ref().map(|node| node.identity.clone()),
        Some(node("src/b.rs"))
    );
    let [from_a, from_c] = incoming.edges.as_slice() else {
        return Err("incoming graph page did not contain exactly two edges".into());
    };
    check_eq!(from_a.edge.source, node("src/a.rs"));
    check_eq!(from_a.edge.target, node("src/b.rs"));
    check_eq!(from_c.edge.source, node("src/c.rs"));
    check_eq!(from_c.edge.target, node("src/b.rs"));
    check_eq!(from_a.supporters, 1);
    check_eq!(from_c.supporters, 1);
    let refusal = query_direction_with_bounds(
        &fixture,
        view,
        node("src/b.rs"),
        NativeGraphDirection::Incoming,
        1,
        1,
    )
    .err()
    .ok_or("incoming query exceeded max_edges but returned a page")?;
    check!(matches!(
        refusal.downcast_ref::<IndexError>(),
        Some(IndexError::ResourceLimited)
    ));
    Ok(())
}

#[test]
fn corrupt_selected_supporter_refuses_query_and_delta_without_publication() -> TestResult {
    let root = tempfile::tempdir()?;
    let fixture = fixture(root.path())?;
    let genesis = fixture
        .kernel
        .create_workspace(workspace(), 1, &mut budget())?;
    let first = contribution(&fixture.objects, 1)?;
    let projection = advance(
        &fixture,
        genesis,
        None,
        1,
        1,
        vec![OwnerMutation {
            owner: owner(1),
            expected: None,
            next: Some(first),
        }],
    )?;
    let selected = configure(&fixture, projection, 2)?;
    assert_support(&query(&fixture, selected)?, 1);

    let artifact = member_artifact(&fixture, selected)?;
    let root_object = fixture.objects.get(artifact, &mut budget())?;
    let encoded: serde_json::Value = serde_json::from_slice(&root_object.payload)?;
    let nodes: MapRoot = serde_json::from_value(
        encoded
            .get("nodes")
            .ok_or("selected graph has no nodes root")?
            .clone(),
    )?;
    let supporter = fixture
        .objects
        .scan(nodes, None, 1, &mut budget())?
        .entries
        .first()
        .ok_or("selected graph has no node supporter")?
        .1;
    let digest = (supporter.object_ref()?.0).0;
    let mut name = String::new();
    for byte in digest {
        write!(&mut name, "{byte:02x}")?;
    }
    let first_byte = digest.first().ok_or("empty graph supporter digest")?;
    let object_file = root
        .path()
        .join("objects")
        .join("objects")
        .join(format!("{first_byte:02x}"))
        .join(name);
    std::fs::write(object_file, b"forged graph supporter")?;
    let query_error = query(&fixture, selected)
        .err()
        .ok_or("corrupt supporter returned a graph page")?;
    check!(query_error.downcast_ref::<IndexError>().is_some());

    let prior = projection_binding(&fixture, current(&fixture)?.projections)?;
    let outcome = advance_outcome(
        &fixture,
        selected,
        Some(&prior),
        3,
        2,
        vec![OwnerMutation {
            owner: owner(1),
            expected: Some(first),
            next: None,
        }],
    )?;
    // CorruptObject is deliberately nonterminal: the original operation stays
    // pending for exact-key recovery after the physical object is repaired.
    check_eq!(outcome, SubmitOutcome::Unknown(operation(3)));
    check_eq!(current(&fixture)?.identity, selected);
    Ok(())
}

#[test]
fn over_bound_affected_fanout_refuses_without_publishing_a_prefix() -> TestResult {
    let root = tempfile::tempdir()?;
    let fixture = fixture(root.path())?;
    let genesis = fixture
        .kernel
        .create_workspace(workspace(), 1, &mut budget())?;
    let first = contribution(&fixture.objects, 1)?;
    let projection = advance(
        &fixture,
        genesis,
        None,
        1,
        1,
        vec![OwnerMutation {
            owner: owner(1),
            expected: None,
            next: Some(first),
        }],
    )?;
    let selected = configure(&fixture, projection, 2)?;
    let prior = projection_binding(&fixture, current(&fixture)?.projections)?;
    let large = GraphOwnerOutput {
        projection: Vec::new(),
        nodes: (0..4096)
            .map(|index| GraphNode {
                identity: node(&format!("src/large{index:04}.rs")),
                payload_digest: Digest([11; 32]),
            })
            .collect(),
        edges: vec![],
    };
    let replacement = contribution_with_output(&fixture.objects, 1, &large)?;
    let outcome = advance_outcome(
        &fixture,
        selected,
        Some(&prior),
        3,
        2,
        vec![OwnerMutation {
            owner: owner(1),
            expected: Some(first),
            next: Some(replacement),
        }],
    )?;
    check!(matches!(
        outcome,
        SubmitOutcome::Terminal(OperationStatus::Refused(_))
    ));
    check_eq!(current(&fixture)?.identity, selected);
    assert_support(&query(&fixture, selected)?, 1);
    Ok(())
}

fn assert_support(page: &GraphPage, expected: u32) {
    assert_eq!(page.source_supporters, expected);
    assert_eq!(page.source.is_some(), expected > 0);
    if expected == 0 {
        assert!(page.edges.is_empty());
    } else {
        assert_eq!(
            page.source.as_ref().map(|node| node.payload_digest),
            Some(Digest([11; 32]))
        );
        let [edge] = page.edges.as_slice() else {
            assert_eq!(page.edges.len(), 1);
            return;
        };
        assert_eq!(edge.edge.kind, RepoMapEdgeKind::DependsOn);
        assert_eq!(edge.edge.source, node("src/a.rs"));
        assert_eq!(edge.edge.target, node("src/b.rs"));
        assert_eq!(edge.supporters, expected);
    }
}

fn assert_incoming_support(page: &GraphPage, expected: u32) {
    assert_eq!(page.source_supporters, expected);
    assert_eq!(page.source.is_some(), expected > 0);
    if expected == 0 {
        assert!(page.edges.is_empty());
    } else {
        assert_eq!(
            page.source.as_ref().map(|node| node.identity.clone()),
            Some(node("src/b.rs"))
        );
        assert_eq!(
            page.source.as_ref().map(|node| node.payload_digest),
            Some(Digest([12; 32]))
        );
        let [edge] = page.edges.as_slice() else {
            assert_eq!(page.edges.len(), 1);
            return;
        };
        assert_eq!(edge.edge.kind, RepoMapEdgeKind::DependsOn);
        assert_eq!(edge.edge.source, node("src/a.rs"));
        assert_eq!(edge.edge.target, node("src/b.rs"));
        assert_eq!(edge.supporters, expected);
    }
}

#[test]
fn certified_graph_support_retracts_and_reappears_without_changing_old_view() -> TestResult {
    let directory = tempfile::tempdir()?;
    let fixture = fixture(directory.path())?;
    let genesis = fixture
        .kernel
        .create_workspace(workspace(), 1, &mut budget())?;
    let first = contribution(&fixture.objects, 1)?;
    let second = contribution(&fixture.objects, 2)?;
    let two = advance(
        &fixture,
        genesis,
        None,
        1,
        1,
        vec![
            OwnerMutation {
                owner: owner(1),
                expected: None,
                next: Some(first),
            },
            OwnerMutation {
                owner: owner(2),
                expected: None,
                next: Some(second),
            },
        ],
    )?;
    let two = configure(&fixture, two, 2)?;
    assert_support(&query(&fixture, two)?, 2);
    assert_incoming_support(&query_incoming(&fixture, two)?, 2);
    let outgoing_b = query_direction_with_bounds(
        &fixture,
        two,
        node("src/b.rs"),
        NativeGraphDirection::Outgoing,
        8,
        8,
    )?;
    check_eq!(outgoing_b.source_supporters, 2);
    check!(outgoing_b.edges.is_empty());
    let retained_two = fixture
        .kernel
        .pin_view(ViewSelection::Exact(two), &budget())?;
    let prior = projection_binding(&fixture, current(&fixture)?.projections)?;
    let one = advance(
        &fixture,
        two,
        Some(&prior),
        3,
        2,
        vec![OwnerMutation {
            owner: owner(1),
            expected: Some(first),
            next: None,
        }],
    )?;
    assert_support(&query(&fixture, one)?, 1);
    assert_support(&query(&fixture, two)?, 2);
    assert_incoming_support(&query_incoming(&fixture, one)?, 1);
    assert_incoming_support(&query_incoming(&fixture, two)?, 2);
    let prior = projection_binding(&fixture, current(&fixture)?.projections)?;
    let unchanged = advance(&fixture, one, Some(&prior), 4, 3, vec![])?;
    // A new certificate changes the retained projection dependencies even
    // when the certified owner set and graph data are unchanged.
    check!(member_artifact(&fixture, one)? != member_artifact(&fixture, unchanged)?);
    let old_outgoing = query(&fixture, one)?;
    let new_outgoing = query(&fixture, unchanged)?;
    assert_support(&new_outgoing, 1);
    check_eq!(old_outgoing, new_outgoing);
    let old_incoming = query_incoming(&fixture, one)?;
    let new_incoming = query_incoming(&fixture, unchanged)?;
    assert_incoming_support(&new_incoming, 1);
    check_eq!(old_incoming, new_incoming);
    let prior = projection_binding(&fixture, current(&fixture)?.projections)?;
    let zero = advance(
        &fixture,
        unchanged,
        Some(&prior),
        5,
        4,
        vec![OwnerMutation {
            owner: owner(2),
            expected: Some(second),
            next: None,
        }],
    )?;
    assert_support(&query(&fixture, zero)?, 0);
    assert_incoming_support(&query_incoming(&fixture, zero)?, 0);
    let prior = projection_binding(&fixture, current(&fixture)?.projections)?;
    let again = advance(
        &fixture,
        zero,
        Some(&prior),
        6,
        5,
        vec![OwnerMutation {
            owner: owner(2),
            expected: None,
            next: Some(second),
        }],
    )?;
    assert_support(&query(&fixture, again)?, 1);
    assert_support(&query(&fixture, retained_two.view().identity)?, 2);
    assert_incoming_support(&query_incoming(&fixture, again)?, 1);
    assert_incoming_support(&query_incoming(&fixture, retained_two.view().identity)?, 2);
    retained_two.release()?;
    drop(fixture);
    let reopened = self::fixture(directory.path())?;
    assert_support(&query(&reopened, again)?, 1);
    assert_incoming_support(&query_incoming(&reopened, again)?, 1);
    let source_advanced = advance_source(&reopened, again)?;
    let error = query(&reopened, source_advanced)
        .err()
        .ok_or("stale graph projection was queried")?;
    check!(matches!(
        error.downcast_ref::<IndexError>(),
        Some(IndexError::SnapshotUnavailable)
    ));
    Ok(())
}
