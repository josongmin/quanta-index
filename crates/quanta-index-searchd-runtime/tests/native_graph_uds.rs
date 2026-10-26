//! Certified graph projection through the public native UDS producer and query paths.
#[path = "common/native_physical_profile.rs"]
mod native_physical_profile;
#[path = "native_graph_uds/ranked.rs"]
mod ranked;

use std::{
    error::Error, os::unix::fs::PermissionsExt, path::Path, thread::JoinHandle, time::Duration,
};

use quanta_index_contract::{
    FileId, RepoMapEdgeKind, RepoMapNodeRef,
    index::{
        CollectionId, Digest, GroupActivation, IndexInstanceId, IndexKind, IndexProfile,
        IngressRef, InputDomain, InstanceSpec, MapRoot, ObjectRef, OperationKey, OperationStatus,
        OwnerKey, OwnerMutation, ProducerCompleteness, ProducerId, ProjectionBinding,
        ProjectionCertificate, ProjectionId, ProjectionOwnerContribution, ProjectionTransition,
        PublicationGroupId, SchemaId, SlotId, SourceEvent, StableOwnerId, StorageFormatId,
        StreamId, SubmitOutcome, TenantId, UpdateKind, UpdateRequest, ViewRef, ViewSelection,
        WorkspaceKey, WorktreeId,
    },
    native_protocol::{
        GraphEdge, GraphNode, GraphOwnerOutput, NativeGraphDirection, NativeGraphQuery,
        NativeGraphResult, NativeIndexErrorCode, NativeIndexRequest, NativeIndexResponse,
    },
};
use quanta_index_core::{
    RequestBudgetV1,
    index::{
        IndexError, IndexQueryPort, MemoryLifetime, QueryOutcome, QueryReadContext, WorkLimits,
        effort::ResourceNeed, ports::IndexProjectionBuildPort,
    },
};
use quanta_index_kernel::{AdapterDescriptor, AdapterRegistry, QueryAdapterRegistry};
use quanta_index_repomap::native::{GraphPage, NativeGraph, NativeGraphReader};
use quanta_index_sdk::native_uds::{NativeUdsClient, NativeUdsError};
use quanta_index_searchd_runtime::native::{
    NativeIndexService, NativeResourcePlan, NativeServiceCapabilities, NativeServiceResources,
};

type TestResult<T = ()> = Result<T, Box<dyn Error>>;
const PRODUCER: ProducerId = ProducerId([41; 16]);
const PROJECTION: ProjectionId = ProjectionId([42; 16]);
const INSTANCE: IndexInstanceId = IndexInstanceId([43; 16]);
const SCHEMA: SchemaId = SchemaId([44; 16]);
const POLICY: Digest = Digest([45; 32]);

fn budget() -> RequestBudgetV1 {
    RequestBudgetV1::for_duration(Duration::from_secs(30))
}

fn work_limits() -> WorkLimits {
    WorkLimits {
        reads: 1_000_000,
        writes: 1_000_000,
        bytes: 512 * 1024 * 1024,
        mutations: 1_000_000,
    }
}

fn resources() -> NativeServiceResources {
    let need = ResourceNeed {
        cpu_units: 1,
        io_bytes: 64 * 1024 * 1024,
        memory_bytes: 64 * 1024 * 1024,
        provider_units: 0,
    };
    NativeServiceResources {
        physical: native_physical_profile::profile(),
        scratch_reserved_bytes: 16 * 1024 * 1024 * 1024,
        read_ticket_lease: Duration::from_secs(30),
        execution: NativeResourcePlan {
            capacity: ResourceNeed {
                cpu_units: 4,
                io_bytes: 256 * 1024 * 1024,
                memory_bytes: 5 * 64 * 1024 * 1024,
                provider_units: 1,
            },
            control: need,
            interactive: need,
            core: need,
            enrichment: ResourceNeed {
                provider_units: 1,
                ..need
            },
        },
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

fn service(root: &Path, socket: &Path) -> TestResult<NativeIndexService> {
    service_with_redirected_reader(root, socket, false)
}

struct RedirectedAnchorReader(NativeGraphReader);

impl IndexQueryPort<NativeGraphQuery, GraphPage> for RedirectedAnchorReader {
    fn validate_instance(&self, instance: &InstanceSpec) -> Result<(), IndexError> {
        self.0.validate_instance(instance)
    }

    fn reservation(&self, query: &NativeGraphQuery) -> Result<ResourceNeed, IndexError> {
        self.0.reservation(query)
    }

    fn query(
        &self,
        request: &NativeGraphQuery,
        context: &mut QueryReadContext<'_>,
    ) -> Result<QueryOutcome<GraphPage>, IndexError> {
        // The wrong reader still returns genuine data from the same pinned
        // view. Raw UDS must reject the changed anchor before wire emission.
        let _memory = context
            .budget
            .reserve_memory(4096, MemoryLifetime::Temporary)?;
        let redirected = NativeGraphQuery {
            source: node("src/b.rs"),
            direction: request.direction,
            max_supporters: request.max_supporters,
            max_edges: request.max_edges,
        };
        self.0.query(&redirected, context)
    }
}

fn service_with_redirected_reader(
    root: &Path,
    socket: &Path,
    redirect: bool,
) -> TestResult<NativeIndexService> {
    service_with_graph_builder(root, socket, redirect, NativeGraph)
}

fn service_with_graph_builder<B: IndexProjectionBuildPort + 'static>(
    root: &Path,
    socket: &Path,
    redirect: bool,
    builder: B,
) -> TestResult<NativeIndexService> {
    let mut builders = AdapterRegistry::new();
    builders.register_projection(descriptor(), builder)?;
    let capabilities = NativeServiceCapabilities {
        builders,
        literal: QueryAdapterRegistry::new(),
        path: QueryAdapterRegistry::new(),
        text: QueryAdapterRegistry::new(),
        semantic: QueryAdapterRegistry::new(),
        pump: None,
        provider_attempt_pool: None,
    };
    let service = NativeIndexService::bind_with_capabilities(
        root,
        socket,
        work_limits(),
        capabilities,
        resources(),
    )?;
    let need = resources().execution.interactive;
    let mut readers: QueryAdapterRegistry<NativeGraphQuery, GraphPage> =
        QueryAdapterRegistry::new();
    if redirect {
        readers.register(
            descriptor(),
            need,
            RedirectedAnchorReader(NativeGraphReader::new(need)?),
        )?;
    } else {
        readers.register(descriptor(), need, NativeGraphReader::new(need)?)?;
    }
    service.configure_graph(readers)?;
    let mut ranked_readers = QueryAdapterRegistry::new();
    ranked_readers.register(
        descriptor(),
        need,
        quanta_index_repomap::native::NativeRepoMapRankReader::new(need)?,
    )?;
    service.configure_repomap(ranked_readers)?;
    Ok(service)
}

struct RunningService {
    shutdown: quanta_index_ipc::ShutdownHandle,
    thread: Option<JoinHandle<anyhow::Result<()>>>,
}

impl RunningService {
    fn start(service: NativeIndexService) -> Self {
        let shutdown = service.shutdown_handle();
        let thread = std::thread::spawn(move || service.run(Duration::from_millis(5)));
        Self {
            shutdown,
            thread: Some(thread),
        }
    }

    fn stop(mut self) -> TestResult {
        self.shutdown.trigger();
        if let Some(thread) = self.thread.take() {
            thread
                .join()
                .map_err(|_panic_payload| "native graph service panicked")??;
        }
        Ok(())
    }
}

impl Drop for RunningService {
    fn drop(&mut self) {
        self.shutdown.trigger();
        if let Some(thread) = self.thread.take() {
            drop(thread.join());
        }
    }
}

fn published(outcome: &SubmitOutcome) -> TestResult<ViewRef> {
    let SubmitOutcome::Terminal(OperationStatus::Published(view)) = outcome else {
        return Err(format!("expected published graph view, got {outcome:?}").into());
    };
    Ok(*view)
}

fn node(path: &str) -> RepoMapNodeRef {
    RepoMapNodeRef::File(FileId::new(path))
}

fn graph_output() -> GraphOwnerOutput {
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

fn stage_owner(
    client: &NativeUdsClient,
    ingress: quanta_index_contract::index::IngressRef,
    id: u8,
) -> TestResult<ObjectRef> {
    let output = client.stage_projection_output(
        ingress,
        &serde_json::to_vec(&graph_output())?,
        &budget(),
    )?;
    Ok(client.stage_projection_owner(
        ingress,
        ProjectionOwnerContribution {
            projection: PROJECTION,
            producer: PRODUCER,
            owner: owner(id),
            source_owner: None,
            source: None,
            dependency_stamp: Digest([id; 32]),
            output: Some(output),
        },
        &budget(),
    )?)
}

fn advance(
    client: &NativeUdsClient,
    view: ViewRef,
    prior: Option<&ProjectionBinding>,
    operation_sequence: u64,
    event_sequence: u64,
    mutations: Vec<OwnerMutation>,
) -> TestResult<ViewRef> {
    let key = operation(operation_sequence);
    let ingress = client.begin_input(key, &budget())?;
    advance_with_ingress(
        client,
        view,
        prior,
        operation_sequence,
        event_sequence,
        mutations,
        ingress,
    )
}

fn advance_with_ingress(
    client: &NativeUdsClient,
    view: ViewRef,
    prior: Option<&ProjectionBinding>,
    operation_sequence: u64,
    event_sequence: u64,
    mutations: Vec<OwnerMutation>,
    ingress: IngressRef,
) -> TestResult<ViewRef> {
    let key = operation(operation_sequence);
    let affected = client.stage_mutations(ingress, mutations, &budget())?;
    let source = client
        .read_view(ViewSelection::Exact(view), &budget())?
        .source;
    let mut transition = ProjectionTransition {
        projection: PROJECTION,
        producer: PRODUCER,
        expected: prior.copied(),
        target_source: source,
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
        projection: PROJECTION,
        producer: PRODUCER,
        prior: prior.copied(),
        target_source: source,
        schema: SCHEMA,
        producer_policy: POLICY,
        dependency_context: transition.dependency_context,
        affected,
    };
    let certificate_ref = client.stage_projection_certificate(ingress, certificate, &budget())?;
    transition.completeness = ProducerCompleteness::CompleteAffectedDomain {
        certificate: certificate_ref,
    };
    transition.events.push(SourceEvent {
        producer: PRODUCER,
        stream: key.stream,
        stream_epoch: 1,
        predecessor: event_sequence
            .checked_sub(1)
            .ok_or("graph event sequence has no predecessor")?,
        sequence: event_sequence,
        payload: transition.payload_digest()?,
    });
    published(&client.submit(
        UpdateRequest {
            operation: key,
            expected_view: view,
            kind: UpdateKind::ProjectionAdvance(Box::new(transition)),
        },
        &budget(),
    )?)
}

fn configure(client: &NativeUdsClient, view: ViewRef) -> TestResult<ViewRef> {
    let key = operation(2);
    let ingress = client.begin_input(key, &budget())?;
    let profile = client.stage_profile(
        ingress,
        IndexProfile {
            instances: vec![spec()],
        },
        &budget(),
    )?;
    published(&client.submit(
        UpdateRequest {
            operation: key,
            expected_view: view,
            kind: UpdateKind::Reconfigure { profile },
        },
        &budget(),
    )?)
}

fn query(client: &NativeUdsClient, view: ViewRef) -> TestResult<NativeGraphResult> {
    query_direction(
        client,
        view,
        node("src/a.rs"),
        NativeGraphDirection::Outgoing,
    )
}

fn query_incoming(client: &NativeUdsClient, view: ViewRef) -> TestResult<NativeGraphResult> {
    query_direction(
        client,
        view,
        node("src/b.rs"),
        NativeGraphDirection::Incoming,
    )
}

fn query_direction(
    client: &NativeUdsClient,
    view: ViewRef,
    source: RepoMapNodeRef,
    direction: NativeGraphDirection,
) -> TestResult<NativeGraphResult> {
    Ok(client.query_graph(
        ViewSelection::Exact(view),
        INSTANCE,
        NativeGraphQuery {
            source,
            direction,
            max_supporters: 8,
            max_edges: 8,
        },
        &budget(),
    )?)
}

fn assert_support(result: &NativeGraphResult, view: ViewRef, expected: u32) {
    assert_eq!(result.view, view);
    assert_eq!(result.instance, INSTANCE);
    assert!(result.exhaustive);
    assert_eq!(result.source_supporters, expected);
    assert_eq!(result.source.is_some(), expected > 0);
    if expected == 0 {
        assert!(result.edges.is_empty());
    } else {
        assert_eq!(
            result.source.as_ref().map(|item| item.payload_digest),
            Some(Digest([11; 32]))
        );
        let [edge] = result.edges.as_slice() else {
            assert_eq!(result.edges.len(), 1);
            return;
        };
        assert_eq!(edge.edge.kind, RepoMapEdgeKind::DependsOn);
        assert_eq!(edge.edge.source, node("src/a.rs"));
        assert_eq!(edge.edge.target, node("src/b.rs"));
        assert_eq!(edge.supporters, expected);
    }
}

fn assert_incoming_support(result: &NativeGraphResult, view: ViewRef, expected: u32) {
    assert_eq!(result.view, view);
    assert_eq!(result.instance, INSTANCE);
    assert!(result.exhaustive);
    assert_eq!(result.source_supporters, expected);
    assert_eq!(result.source.is_some(), expected > 0);
    if expected == 0 {
        assert!(result.edges.is_empty());
    } else {
        assert_eq!(
            result.source.as_ref().map(|item| item.identity.clone()),
            Some(node("src/b.rs"))
        );
        assert_eq!(
            result.source.as_ref().map(|item| item.payload_digest),
            Some(Digest([12; 32]))
        );
        let [edge] = result.edges.as_slice() else {
            assert_eq!(result.edges.len(), 1);
            return;
        };
        assert_eq!(edge.edge.kind, RepoMapEdgeKind::DependsOn);
        assert_eq!(edge.edge.source, node("src/a.rs"));
        assert_eq!(edge.edge.target, node("src/b.rs"));
        assert_eq!(edge.supporters, expected);
    }
}

#[test]
fn certified_graph_projection_support_delta_query_old_view_and_reopen() -> TestResult {
    let root = tempfile::tempdir()?;
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))?;
    let socket = root.path().join("native.sock");
    let running = RunningService::start(service(root.path(), &socket)?);
    let client = NativeUdsClient::new(socket.clone());
    let NativeIndexResponse::Created(genesis) = client.request(
        NativeIndexRequest::CreateWorkspace {
            workspace: workspace(),
            epoch: 1,
        },
        &budget(),
    )?
    else {
        return Err("graph workspace creation did not return a view".into());
    };

    let key = operation(1);
    let ingress = client.begin_input(key, &budget())?;
    let first = stage_owner(&client, ingress, 1)?;
    let second = stage_owner(&client, ingress, 2)?;
    let affected = client.stage_mutations(
        ingress,
        [
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
        &budget(),
    )?;
    let source = client
        .read_view(ViewSelection::Exact(genesis), &budget())?
        .source;
    let mut transition = ProjectionTransition {
        projection: PROJECTION,
        producer: PRODUCER,
        expected: None,
        target_source: source,
        schema: SCHEMA,
        producer_policy: POLICY,
        dependency_context: Digest([50; 32]),
        completeness: ProducerCompleteness::Partial {
            missing: MapRoot(None),
        },
        owners: affected,
        events: Vec::new(),
    };
    let certificate_ref = client.stage_projection_certificate(
        ingress,
        ProjectionCertificate {
            projection: PROJECTION,
            producer: PRODUCER,
            prior: None,
            target_source: source,
            schema: SCHEMA,
            producer_policy: POLICY,
            dependency_context: transition.dependency_context,
            affected,
        },
        &budget(),
    )?;
    transition.completeness = ProducerCompleteness::CompleteAffectedDomain {
        certificate: certificate_ref,
    };
    transition.events.push(SourceEvent {
        producer: PRODUCER,
        stream: key.stream,
        stream_epoch: 1,
        predecessor: 0,
        sequence: 1,
        payload: transition.payload_digest()?,
    });
    let two = published(&client.submit(
        UpdateRequest {
            operation: key,
            expected_view: genesis,
            kind: UpdateKind::ProjectionAdvance(Box::new(transition)),
        },
        &budget(),
    )?)?;
    let two = configure(&client, two)?;
    assert_support(&query(&client, two)?, two, 2);
    assert_incoming_support(&query_incoming(&client, two)?, two, 2);
    let outgoing_b = query_direction(
        &client,
        two,
        node("src/b.rs"),
        NativeGraphDirection::Outgoing,
    )?;
    if outgoing_b.source_supporters != 2 || !outgoing_b.edges.is_empty() {
        return Err("graph target has an unexpected outgoing edge".into());
    }

    let prior = client.read_projection_binding(ViewSelection::Exact(two), PROJECTION, &budget())?;
    if prior.view != two {
        return Err("first projection CAS read selected a different view".into());
    }
    let one = advance(
        &client,
        two,
        Some(
            prior
                .binding
                .as_ref()
                .ok_or("first certified projection binding absent")?,
        ),
        3,
        2,
        vec![OwnerMutation {
            owner: owner(1),
            expected: Some(first),
            next: None,
        }],
    )?;
    assert_support(&query(&client, one)?, one, 1);
    assert_support(&query(&client, two)?, two, 2);
    assert_incoming_support(&query_incoming(&client, one)?, one, 1);
    assert_incoming_support(&query_incoming(&client, two)?, two, 2);

    let prior = client.read_projection_binding(ViewSelection::Exact(one), PROJECTION, &budget())?;
    if prior.view != one {
        return Err("second projection CAS read selected a different view".into());
    }
    let zero = advance(
        &client,
        one,
        Some(
            prior
                .binding
                .as_ref()
                .ok_or("second certified projection binding absent")?,
        ),
        4,
        3,
        vec![OwnerMutation {
            owner: owner(2),
            expected: Some(second),
            next: None,
        }],
    )?;
    assert_support(&query(&client, zero)?, zero, 0);
    assert_support(&query(&client, two)?, two, 2);
    assert_incoming_support(&query_incoming(&client, zero)?, zero, 0);
    assert_incoming_support(&query_incoming(&client, two)?, two, 2);

    let prior =
        client.read_projection_binding(ViewSelection::Exact(zero), PROJECTION, &budget())?;
    if prior.view != zero {
        return Err("third projection CAS read selected a different view".into());
    }
    let ingress = client.begin_input(operation(5), &budget())?;
    let restaged = stage_owner(&client, ingress, 2)?;
    if restaged != second {
        return Err("recreated graph owner changed its certified object".into());
    }
    let again = advance_with_ingress(
        &client,
        zero,
        Some(
            prior
                .binding
                .as_ref()
                .ok_or("third certified projection binding absent")?,
        ),
        5,
        4,
        vec![OwnerMutation {
            owner: owner(2),
            expected: None,
            next: Some(restaged),
        }],
        ingress,
    )?;
    assert_support(&query(&client, again)?, again, 1);
    assert_incoming_support(&query_incoming(&client, again)?, again, 1);
    assert_incoming_support(&query_incoming(&client, two)?, two, 2);

    running.stop()?;
    let reopened = RunningService::start(service(root.path(), &socket)?);
    let client = NativeUdsClient::new(socket.clone());
    assert_support(&query(&client, zero)?, zero, 0);
    assert_support(&query(&client, two)?, two, 2);
    assert_support(&query(&client, again)?, again, 1);
    assert_incoming_support(&query_incoming(&client, zero)?, zero, 0);
    assert_incoming_support(&query_incoming(&client, two)?, two, 2);
    assert_incoming_support(&query_incoming(&client, again)?, again, 1);
    reopened.stop()?;

    let redirected =
        RunningService::start(service_with_redirected_reader(root.path(), &socket, true)?);
    let client = NativeUdsClient::new(socket);
    for direction in [
        NativeGraphDirection::Outgoing,
        NativeGraphDirection::Incoming,
    ] {
        let result = client.request(
            NativeIndexRequest::QueryGraph {
                selection: ViewSelection::Exact(again),
                instance: INSTANCE,
                query: NativeGraphQuery {
                    source: node("src/a.rs"),
                    direction,
                    max_supporters: 8,
                    max_edges: 8,
                },
            },
            &budget(),
        );
        if !matches!(result, Err(NativeUdsError::Refused(error))
            if error.code == NativeIndexErrorCode::BackendViolation)
        {
            return Err("raw UDS exposed a graph for a different request anchor".into());
        }
    }
    redirected.stop()?;
    Ok(())
}
