//! Certified graph projection through the public native UDS producer and query paths.
#[path = "common/native_object_files.rs"]
mod native_object_files;
#[path = "common/native_physical_profile.rs"]
mod native_physical_profile;

use std::sync::{Arc, Mutex};
use std::{
    error::Error, os::unix::fs::PermissionsExt, path::Path, thread::JoinHandle, time::Duration,
};

use quanta_index_catalog::SqliteCatalog;
use quanta_index_contract::{
    FileId, RepoMapEdgeKind, RepoMapNodeRef,
    index::{
        CollectionId, Digest, GroupActivation, IndexInstanceId, IndexKind, IndexProfile,
        IngressRef, InputDomain, InstanceSpec, MapRoot, ObjectRef, OperationKey, OperationStatus,
        OwnerKey, OwnerMutation, ProducerCompleteness, ProducerId, ProjectionBinding,
        ProjectionCertificate, ProjectionId, ProjectionOwnerContribution, ProjectionTransition,
        PublicationGroupId, Refusal, SchemaId, SlotId, SourceEvent, StableOwnerId, StorageFormatId,
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
        AttemptContext, CatalogView, IndexError, IndexQueryPort, MemoryLifetime, QueryOutcome,
        QueryReadContext, WorkBudget, WorkLimits, decode_member_binding,
        effort::ResourceNeed,
        member_map_key,
        ports::{
            CatalogExactHistoryPort, CatalogReadPort, ImmutableObjectPort, IndexFuture,
            IndexProjectionBuildPort, PersistentMapPort,
        },
    },
};
use quanta_index_kernel::{AdapterDescriptor, AdapterRegistry, QueryAdapterRegistry};
use quanta_index_repomap::native::graph::GraphCandidate;
use quanta_index_repomap::native::{GraphPage, NativeGraph, NativeGraphReader};
use quanta_index_sdk::native_uds::{NativeUdsClient, NativeUdsError};
use quanta_index_searchd_runtime::native::{
    NativeIndexService, NativeReclaimConfig, NativeResourcePlan, NativeRetentionConfig,
    NativeRetentionScope, NativeServiceCapabilities, NativeServiceResources,
    NativeWalReclaimPolicy,
};
use quanta_index_storage::FileObjectStore;

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
        catalog_writer_cache: quanta_index_searchd_runtime::native::CatalogWriterCachePolicy {
            kib: 2048,
        },
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
    service_with_graph_builder_retention(root, socket, redirect, builder, false)
}

fn service_with_graph_builder_retention<B: IndexProjectionBuildPort + 'static>(
    root: &Path,
    socket: &Path,
    redirect: bool,
    builder: B,
    retention: bool,
) -> TestResult<NativeIndexService> {
    let mut builders = AdapterRegistry::new();
    builders.register_projection(descriptor(), builder)?;
    let capabilities = NativeServiceCapabilities {
        builders,
        literal: QueryAdapterRegistry::new(),
        path: QueryAdapterRegistry::new(),
        text: QueryAdapterRegistry::new(),
        text_matches: QueryAdapterRegistry::new(),
        semantic: QueryAdapterRegistry::new(),
        pump: None,
        provider_attempt_pool: None,
    };
    let mut funded = resources();
    let service = if retention {
        funded.execution.capacity.cpu_units = 5;
        funded.execution.capacity.io_bytes = 320 * 1024 * 1024;
        funded.execution.capacity.memory_bytes = 6 * 64 * 1024 * 1024;
        NativeIndexService::bind_with_retention(
            root,
            socket,
            work_limits(),
            capabilities,
            &funded,
            Some(NativeReclaimConfig {
                interval: Duration::from_millis(20),
                wal_reclaim: NativeWalReclaimPolicy {
                    trigger_bytes: 8 * 1024 * 1024,
                    min_interval: Duration::from_secs(1),
                },
                // Retention requires one round to reach every root a retired
                // view queues: seven steps and their round byte allowance.
                max_steps_per_round: 7,
                child_limit: 32,
                work_limits: WorkLimits {
                    bytes: 939_524_096,
                    ..work_limits()
                },
                round_timeout: Duration::from_secs(2),
                head_probe_every: 4,
            }),
            Some(NativeRetentionConfig {
                scopes: vec![NativeRetentionScope {
                    workspace: workspace(),
                    epoch: 1,
                }],
                sequence_window: 1,
                page_limit: 8,
                custody_page_limit: 8,
                interval: Duration::from_millis(20),
                round_timeout: Duration::from_secs(2),
                work_limits: work_limits(),
                head_probe_every: 2,
            }),
        )?
    } else {
        NativeIndexService::bind_with_capabilities(
            root,
            socket,
            work_limits(),
            capabilities,
            &funded,
        )?
    };
    let need = funded.execution.interactive;
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
        facts: None,
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

fn stage_graph_owner_output(
    client: &NativeUdsClient,
    ingress: IngressRef,
    id: u8,
    graph: &GraphOwnerOutput,
    request_budget: &RequestBudgetV1,
) -> TestResult<ObjectRef> {
    let output =
        client.stage_projection_output(ingress, &serde_json::to_vec(graph)?, request_budget)?;
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
        request_budget,
    )?)
}

fn graph_output_with_node_count(count: usize, prefix: &str) -> GraphOwnerOutput {
    GraphOwnerOutput {
        projection: Vec::new(),
        facts: None,
        nodes: (0..count)
            .map(|index| GraphNode {
                identity: node(&format!("{prefix}/{index:04}.rs")),
                payload_digest: Digest([13; 32]),
            })
            .collect(),
        edges: Vec::new(),
    }
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
    published(&submit_advance_with_ingress(
        client,
        view,
        prior,
        operation_sequence,
        event_sequence,
        mutations,
        ingress,
        &budget(),
    )?)
}

fn submit_advance_with_ingress(
    client: &NativeUdsClient,
    view: ViewRef,
    prior: Option<&ProjectionBinding>,
    operation_sequence: u64,
    event_sequence: u64,
    mutations: Vec<OwnerMutation>,
    ingress: IngressRef,
    request_budget: &RequestBudgetV1,
) -> TestResult<SubmitOutcome> {
    let key = operation(operation_sequence);
    let affected = client.stage_mutations(ingress, mutations, request_budget)?;
    let source = client
        .read_view(ViewSelection::Exact(view), request_budget)?
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
    let certificate_ref =
        client.stage_projection_certificate(ingress, certificate, request_budget)?;
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
    Ok(client.submit(
        UpdateRequest {
            operation: key,
            expected_view: view,
            kind: UpdateKind::ProjectionAdvance(Box::new(transition)),
        },
        request_budget,
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

/// Test-only read of the authenticated member's immutable graph artifact.
/// The service is stopped while this standalone store view is open.
fn graph_physical_roots(
    root: &Path,
    view: &CatalogView,
) -> TestResult<(ObjectRef, Option<ObjectRef>)> {
    let objects = FileObjectStore::open(root.join("native-objects"))?;
    let mut work = WorkBudget::with_request(work_limits(), budget());
    let member_ref = objects
        .lookup_object(view.members, &member_map_key(INSTANCE), &mut work)?
        .ok_or("graph member absent from exact view")?;
    let member_wire = objects.get(member_ref, &mut work)?;
    let member = decode_member_binding(&member_wire.payload, &mut work)?;
    if member.instance != INSTANCE {
        return Err("graph member instance differs from exact view".into());
    }
    let graph_wire = objects.get(member.artifact, &mut work)?;
    let root: serde_json::Value = serde_json::from_slice(&graph_wire.payload)?;
    let nodes: MapRoot = serde_json::from_value(
        root.get("nodes")
            .ok_or("graph root lacks node map")?
            .clone(),
    )?;
    Ok((member.artifact, nodes.0))
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

    let old_view = client.read_view(ViewSelection::Exact(two), &budget())?;
    let shared_view = client.read_view(ViewSelection::Exact(one), &budget())?;
    let empty_view = client.read_view(ViewSelection::Exact(zero), &budget())?;
    let live_view = client.read_view(ViewSelection::Exact(again), &budget())?;

    running.stop()?;
    let (old_graph, _) = graph_physical_roots(root.path(), &old_view)?;
    let (_, shared_file_map) = graph_physical_roots(root.path(), &shared_view)?;
    let (empty_graph, empty_nodes) = graph_physical_roots(root.path(), &empty_view)?;
    let (live_graph, live_file_map) = graph_physical_roots(root.path(), &live_view)?;
    if old_graph == live_graph
        || empty_graph == old_graph
        || empty_graph == live_graph
        || empty_nodes.is_some()
        || shared_file_map != live_file_map
    {
        return Err("graph fixture lacks distinct roots and a reused node map".into());
    }
    let shared_file_map = shared_file_map.ok_or("graph fixture has no shared node map")?;
    let old_graph_file =
        native_object_files::object_file(&root.path().join("native-objects"), old_graph)?;
    let empty_graph_file =
        native_object_files::object_file(&root.path().join("native-objects"), empty_graph)?;
    let shared_map_file =
        native_object_files::object_file(&root.path().join("native-objects"), shared_file_map)?;
    if !old_graph_file.is_file() || !empty_graph_file.is_file() || !shared_map_file.is_file() {
        return Err("graph fixture physical objects are absent before retention".into());
    }
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
    let client = NativeUdsClient::new(&socket);
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

    // Hold the old exact view across the real Catalog retirement and object
    // reclaim pumps. Other below-floor history can retire while this lease
    // still keeps the old graph root's physical object reachable.
    let catalog = SqliteCatalog::open(root.path(), Duration::from_secs(2))?;
    let session = catalog.open_read_session()?;
    let pin = catalog.acquire_ticket(
        session.as_ref(),
        ViewSelection::Exact(two),
        Duration::from_secs(30),
        &WorkBudget::with_request(work_limits(), budget()),
    )?;
    if pin.view.identity != two {
        return Err("graph retention pin selected another exact view".into());
    }
    let retained = RunningService::start(service_with_graph_builder_retention(
        root.path(),
        &socket,
        false,
        NativeGraph,
        true,
    )?);
    let retained_client = NativeUdsClient::new(socket.clone());
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while (catalog.exact_history_floor(workspace(), 1)? != again.sequence
        || !native_object_files::absent_nofollow(&empty_graph_file)?
        || !matches!(
            retained_client.read_view(ViewSelection::Exact(one), &budget()),
            Err(NativeUdsError::Refused(error))
                if error.code == NativeIndexErrorCode::SnapshotUnavailable
        ))
        && std::time::Instant::now() < deadline
    {
        std::thread::sleep(Duration::from_millis(20));
    }
    if catalog.exact_history_floor(workspace(), 1)? != again.sequence
        || !native_object_files::absent_nofollow(&empty_graph_file)?
        || !matches!(
            retained_client.read_view(ViewSelection::Exact(one), &budget()),
            Err(NativeUdsError::Refused(error))
                if error.code == NativeIndexErrorCode::SnapshotUnavailable
        )
    {
        return Err("graph retention did not physically reclaim an unpinned root".into());
    }
    if native_object_files::absent_nofollow(&old_graph_file)?
        || native_object_files::absent_nofollow(&shared_map_file)?
    {
        return Err("graph reclaim removed an exact-pinned or shared node map object".into());
    }
    retained.stop()?;
    catalog.validate_ticket(session.as_ref(), pin.lease)?;
    catalog.release_ticket(session.as_ref(), pin.lease)?;
    drop(session);
    drop(catalog);

    let reclaimed = RunningService::start(service_with_graph_builder_retention(
        root.path(),
        &socket,
        false,
        NativeGraph,
        true,
    )?);
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while !native_object_files::absent_nofollow(&old_graph_file)?
        && std::time::Instant::now() < deadline
    {
        std::thread::sleep(Duration::from_millis(20));
    }
    if !native_object_files::absent_nofollow(&old_graph_file)? {
        return Err("retired graph root remained after the exact pin released".into());
    }
    if native_object_files::absent_nofollow(&shared_map_file)? {
        return Err("reclaim removed the current graph's shared node map".into());
    }
    assert_support(&query(&NativeUdsClient::new(socket), again)?, again, 1);
    reclaimed.stop()?;
    Ok(())
}

// Independent Graph capacity regressions retained after RepoMap rank removal.

struct ObservedGraph {
    inner: NativeGraph,
    first_error: FirstGraphFailure,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct GraphFailure {
    stage: &'static str,
    error: IndexError,
}

type FirstGraphFailure = Arc<Mutex<Option<GraphFailure>>>;

impl ObservedGraph {
    fn capture(&self, stage: &'static str, error: &IndexError) {
        if let Ok(mut first) = self.first_error.lock()
            && first.is_none()
        {
            *first = Some(GraphFailure {
                stage,
                error: error.clone(),
            });
        }
    }
}

impl IndexProjectionBuildPort for ObservedGraph {
    type Candidate = GraphCandidate;

    fn resource_demand(&self) -> ResourceNeed {
        self.inner.resource_demand()
    }

    fn validate_instance(&self, instance: &InstanceSpec) -> Result<(), IndexError> {
        let result = self.inner.validate_instance(instance);
        if let Err(error) = &result {
            self.capture("validate_instance", error);
        }
        result
    }

    fn prepare_projection<'a>(
        &'a self,
        instance: &'a InstanceSpec,
        prior: Option<quanta_index_contract::index::MemberBinding>,
        projection: ProjectionBinding,
        affected: Option<quanta_index_contract::index::OwnerMutationSet>,
        context: &'a mut AttemptContext<'_>,
    ) -> IndexFuture<'a, Self::Candidate> {
        Box::pin(async move {
            let result = self
                .inner
                .prepare_projection(instance, prior, projection, affected, context)
                .await;
            if let Err(error) = &result {
                self.capture("prepare_projection", error);
            }
            result
        })
    }

    fn verify_projection<'a>(
        &'a self,
        candidate: Self::Candidate,
        context: &'a mut AttemptContext<'_>,
    ) -> IndexFuture<'a, quanta_index_contract::index::MemberBinding> {
        Box::pin(async move {
            let result = self.inner.verify_projection(candidate, context).await;
            if let Err(error) = &result {
                self.capture("verify_projection", error);
            }
            result
        })
    }
}

fn observed_service(
    root: &Path,
    socket: &Path,
    first_error: FirstGraphFailure,
) -> TestResult<NativeIndexService> {
    service_with_graph_builder(
        root,
        socket,
        false,
        ObservedGraph {
            inner: NativeGraph,
            first_error,
        },
    )
}

fn capacity_budget() -> RequestBudgetV1 {
    RequestBudgetV1::for_duration(Duration::from_secs(180))
}

fn assert_graph_capacity_failure(
    client: &NativeUdsClient,
    first_error: &FirstGraphFailure,
    key: OperationKey,
    retained: ViewRef,
    outcome: SubmitOutcome,
) -> TestResult {
    if outcome != SubmitOutcome::Terminal(OperationStatus::Refused(Refusal::ResourceLimited)) {
        return Err(format!("graph capacity attempt returned {outcome:?}").into());
    }
    let observed = first_error
        .lock()
        .map_err(|_poisoned| "graph capacity observer lock poisoned")?;
    if !matches!(
        observed.as_ref(),
        Some(GraphFailure {
            stage: "prepare_projection",
            error: IndexError::ResourceLimited,
        })
    ) {
        return Err(format!("graph capacity had another builder outcome: {observed:?}").into());
    }
    drop(observed);
    if client.status(key, &budget())? != OperationStatus::Refused(Refusal::ResourceLimited) {
        return Err("graph capacity attempt lacks its terminal resource refusal".into());
    }
    let latest = client.read_view(
        ViewSelection::Latest {
            workspace: workspace(),
            epoch: 1,
        },
        &budget(),
    )?;
    if latest.identity != retained {
        return Err("graph capacity failure advanced the published view".into());
    }
    Ok(())
}

#[test]
fn graph_bootstrap_refuses_two_individually_valid_2049_node_owners() -> TestResult {
    let root = tempfile::tempdir()?;
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))?;
    let socket = root.path().join("native.sock");
    let first_error = Arc::new(Mutex::new(None));
    let running = RunningService::start(observed_service(
        root.path(),
        &socket,
        Arc::clone(&first_error),
    )?);
    let client = NativeUdsClient::new(socket);
    let build_budget = capacity_budget();
    let NativeIndexResponse::Created(genesis) = client.request(
        NativeIndexRequest::CreateWorkspace {
            workspace: workspace(),
            epoch: 1,
        },
        &budget(),
    )?
    else {
        return Err("graph capacity workspace creation returned no view".into());
    };
    let ingress = client.begin_input(operation(1), &budget())?;
    let first = stage_graph_owner_output(
        &client,
        ingress,
        1,
        &graph_output_with_node_count(2049, "src/capacity/first"),
        &build_budget,
    )?;
    let second = stage_graph_owner_output(
        &client,
        ingress,
        2,
        &graph_output_with_node_count(2049, "src/capacity/second"),
        &build_budget,
    )?;
    let projection = published(&submit_advance_with_ingress(
        &client,
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
        ingress,
        &build_budget,
    )?)?;
    let ingress = client.begin_input(operation(2), &budget())?;
    let profile = client.stage_profile(
        ingress,
        IndexProfile {
            instances: vec![spec()],
        },
        &build_budget,
    )?;
    let outcome = client.submit(
        UpdateRequest {
            operation: operation(2),
            expected_view: projection,
            kind: UpdateKind::Reconfigure { profile },
        },
        &build_budget,
    )?;
    assert_graph_capacity_failure(&client, &first_error, operation(2), projection, outcome)?;
    running.stop()
}

#[test]
fn graph_delta_refuses_old_plus_new_above_4096_and_retains_old_view() -> TestResult {
    let root = tempfile::tempdir()?;
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))?;
    let socket = root.path().join("native.sock");
    let first_error = Arc::new(Mutex::new(None));
    let running = RunningService::start(observed_service(
        root.path(),
        &socket,
        Arc::clone(&first_error),
    )?);
    let client = NativeUdsClient::new(socket);
    let build_budget = capacity_budget();
    let NativeIndexResponse::Created(genesis) = client.request(
        NativeIndexRequest::CreateWorkspace {
            workspace: workspace(),
            epoch: 1,
        },
        &budget(),
    )?
    else {
        return Err("graph capacity workspace creation returned no view".into());
    };
    let ingress = client.begin_input(operation(1), &budget())?;
    let first = stage_graph_owner_output(
        &client,
        ingress,
        1,
        &graph_output_with_node_count(2048, "src/capacity"),
        &build_budget,
    )?;
    let projection = published(&submit_advance_with_ingress(
        &client,
        genesis,
        None,
        1,
        1,
        vec![OwnerMutation {
            owner: owner(1),
            expected: None,
            next: Some(first),
        }],
        ingress,
        &build_budget,
    )?)?;
    let ingress = client.begin_input(operation(2), &budget())?;
    let profile = client.stage_profile(
        ingress,
        IndexProfile {
            instances: vec![spec()],
        },
        &budget(),
    )?;
    let active = published(&client.submit(
        UpdateRequest {
            operation: operation(2),
            expected_view: projection,
            kind: UpdateKind::Reconfigure { profile },
        },
        &build_budget,
    )?)?;
    let old_node = node("src/capacity/0000.rs");
    let new_node = node("src/capacity/2048.rs");
    let before = query_direction(
        &client,
        active,
        old_node.clone(),
        NativeGraphDirection::Outgoing,
    )?;
    if before.source_supporters != 1 {
        return Err("graph capacity fixture has no published old node".into());
    }
    let prior =
        client.read_projection_binding(ViewSelection::Exact(active), PROJECTION, &budget())?;
    let ingress = client.begin_input(operation(3), &budget())?;
    let replacement = stage_graph_owner_output(
        &client,
        ingress,
        1,
        &graph_output_with_node_count(2049, "src/capacity"),
        &build_budget,
    )?;
    let outcome = submit_advance_with_ingress(
        &client,
        active,
        Some(
            prior
                .binding
                .as_ref()
                .ok_or("graph capacity prior binding absent")?,
        ),
        3,
        2,
        vec![OwnerMutation {
            owner: owner(1),
            expected: Some(first),
            next: Some(replacement),
        }],
        ingress,
        &build_budget,
    )?;
    assert_graph_capacity_failure(&client, &first_error, operation(3), active, outcome)?;
    let after = query_direction(&client, active, old_node, NativeGraphDirection::Outgoing)?;
    let absent = query_direction(&client, active, new_node, NativeGraphDirection::Outgoing)?;
    if after != before || absent.source_supporters != 0 || absent.source.is_some() {
        return Err("failed graph delta changed the exact old view".into());
    }
    running.stop()
}
