//! Native source-owner lookup through the real service, UDS codec, and SDK.

#[path = "common/native_physical_profile.rs"]
mod native_physical_profile;

use std::{error::Error, os::unix::fs::PermissionsExt, thread::JoinHandle, time::Duration};

use quanta_index_contract::{
    index::{
        CollectionId, Digest, GroupActivation, IndexInstanceId, IndexKind, IndexProfile,
        IngressRef, InputDomain, InstanceSpec, ObjectRef, OperationKey, OperationStatus, OwnerKey,
        OwnerMutation, ProducerId, PublicationGroupId, SchemaId, SlotId, SourceCut, SourceEvent,
        SourceTextAdmission, SourceTransition, StableOwnerId, StorageFormatId, StreamId,
        SubmitOutcome, TenantId, UpdateKind, UpdateRequest, ViewRef, ViewSelection, WorkspaceKey,
        WorktreeId,
    },
    native_protocol::{
        NativeIndexErrorCode, NativeIndexRequest, NativeIndexResponse, NativeSourceOwnerLookup,
    },
};
use quanta_index_core::{
    RequestBudgetV1,
    index::{WorkLimits, effort::ResourceNeed},
};
use quanta_index_kernel::AdapterDescriptor;
use quanta_index_sdk::native_uds::{NativeUdsClient, NativeUdsError};
use quanta_index_searchd_runtime::native::{
    NativeAdapterDescriptors, NativeIndexService, NativeResourceObservation, NativeResourcePlan,
    NativeServiceResources,
};

type TestResult<T = ()> = Result<T, Box<dyn Error>>;

macro_rules! check {
    ($condition:expr, $message:literal) => {
        if !$condition {
            return Err($message.into());
        }
    };
}

macro_rules! check_eq {
    ($actual:expr, $expected:expr, $message:literal) => {
        if $actual != $expected {
            return Err($message.into());
        }
    };
}

#[path = "native_source_owner_lookup/projection_source_diff.rs"]
mod projection_source_diff;

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
        producer: ProducerId([5; 16]),
        stream: StreamId([6; 16]),
        stream_epoch: 1,
        sequence,
    }
}

fn owner(id: u8) -> OwnerKey {
    OwnerKey {
        producer: operation(1).producer,
        stable_id: StableOwnerId([id; 16]),
    }
}

fn instance(id: u8) -> InstanceSpec {
    InstanceSpec {
        instance: IndexInstanceId([id; 16]),
        kind: IndexKind([id; 16]),
        group: PublicationGroupId([9; 16]),
        activation: GroupActivation::CoreRequired,
        input_domain: InputDomain::SourceLocal,
        schema: SchemaId([10; 16]),
        policy: Digest([id; 32]),
        format: StorageFormatId([id; 16]),
    }
}

fn descriptors() -> NativeAdapterDescriptors {
    let literal = instance(20);
    let file_map = instance(30);
    NativeAdapterDescriptors {
        literal: AdapterDescriptor {
            kind: literal.kind,
            schema: literal.schema,
            format: literal.format,
        },
        file_map: AdapterDescriptor {
            kind: file_map.kind,
            schema: file_map.schema,
            format: file_map.format,
        },
    }
}

fn limits() -> WorkLimits {
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

fn budget() -> RequestBudgetV1 {
    RequestBudgetV1::for_duration(Duration::from_secs(30))
}

fn published(outcome: SubmitOutcome) -> TestResult<ViewRef> {
    match outcome {
        SubmitOutcome::Terminal(OperationStatus::Published(view)) => Ok(view),
        other @ (SubmitOutcome::Accepted(_)
        | SubmitOutcome::Terminal(_)
        | SubmitOutcome::Unknown(_)) => {
            Err(format!("expected published native view, got {other:?}").into())
        }
    }
}

struct RunningService {
    shutdown: quanta_index_ipc::ShutdownHandle,
    thread: Option<JoinHandle<anyhow::Result<()>>>,
    resources: NativeResourceObservation,
}

impl RunningService {
    fn start(service: NativeIndexService) -> Self {
        let shutdown = service.shutdown_handle();
        let resources = service.resource_observation();
        let thread = std::thread::spawn(move || service.run(Duration::from_millis(5)));
        Self {
            shutdown,
            thread: Some(thread),
            resources,
        }
    }

    fn stop(mut self) -> TestResult {
        self.shutdown.trigger();
        if let Some(thread) = self.thread.take() {
            thread
                .join()
                .map_err(|_panic| "native service thread panicked")??;
        }
        check_eq!(
            self.resources.observe()?.reserved,
            ResourceNeed::default(),
            "source lookup leaked an execution reservation"
        );
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

fn advance(
    client: &NativeUdsClient,
    ingress: IngressRef,
    expected_view: ViewRef,
    previous: Option<ObjectRef>,
    next: Option<ObjectRef>,
    event_sequence: u64,
) -> TestResult<ViewRef> {
    let source = client
        .read_view(ViewSelection::Exact(expected_view), &budget())?
        .source;
    let owners = client.stage_mutations(
        ingress,
        [OwnerMutation {
            owner: owner(1),
            expected: previous,
            next,
        }],
        &budget(),
    )?;
    let mut transition = SourceTransition {
        expected_source: source,
        owners,
        events: Vec::new(),
    };
    transition.events.push(SourceEvent {
        producer: operation(1).producer,
        stream: operation(1).stream,
        stream_epoch: 1,
        predecessor: event_sequence
            .checked_sub(1)
            .ok_or("source event predecessor underflow")?,
        sequence: event_sequence,
        payload: transition.owner_digest()?,
    });
    published(client.submit(
        UpdateRequest {
            operation: ingress.operation,
            expected_view,
            kind: UpdateKind::SourceAdvance(transition),
        },
        &budget(),
    )?)
}

fn check_lookup(
    response: &NativeSourceOwnerLookup,
    expected_view: ViewRef,
    expected_source: SourceCut,
    expected_record: Option<ObjectRef>,
) -> TestResult {
    check_eq!(response.view, expected_view, "lookup view changed");
    check_eq!(
        response.source,
        expected_source,
        "lookup source cut changed"
    );
    let rows = response.rows.as_slice();
    check_eq!(rows.len(), 2, "lookup row cardinality changed");
    let selected = rows.first().ok_or("selected owner absent")?;
    check_eq!(selected.owner, owner(1), "lookup owner order changed");
    check_eq!(
        selected.source,
        expected_record,
        "lookup owner value changed"
    );
    let missing = rows.get(1).ok_or("missing-owner row absent")?;
    check_eq!(missing.owner, owner(9), "missing owner order changed");
    check_eq!(missing.source, None, "missing owner fabricated a source");
    Ok(())
}

fn run_owner_lookup_case() -> TestResult {
    let root = tempfile::tempdir()?;
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))?;
    let socket = root.path().join("native.sock");
    let service =
        NativeIndexService::bind(root.path(), &socket, limits(), descriptors(), resources())?;
    let running = RunningService::start(service);
    let client = NativeUdsClient::new(socket);
    let created = client.request(
        NativeIndexRequest::CreateWorkspace {
            workspace: workspace(),
            epoch: 1,
        },
        &budget(),
    )?;
    let NativeIndexResponse::Created(genesis) = created else {
        return Err(format!("unexpected genesis response: {created:?}").into());
    };
    let ingress = client.begin_input(operation(1), &budget())?;
    let profile = client.stage_profile(
        ingress,
        IndexProfile {
            instances: vec![instance(20), instance(30)],
        },
        &budget(),
    )?;
    let configured = published(client.submit(
        UpdateRequest {
            operation: operation(1),
            expected_view: genesis,
            kind: UpdateKind::Reconfigure { profile },
        },
        &budget(),
    )?)?;

    let ingress_a = client.begin_input(operation(2), &budget())?;
    let record_a = client.stage_source(
        ingress_a,
        "first.rs".into(),
        b"first source",
        SourceTextAdmission::Excluded,
        &budget(),
    )?;
    let view_a = advance(&client, ingress_a, configured, None, Some(record_a), 1)?;

    // Each Exact lookup below acquires its ticket through the admitted service.
    // Retention is not running in this lookup fixture. Do not open a second raw
    // writer into the managed Catalog namespace to fabricate a held pin.
    let ingress_b = client.begin_input(operation(3), &budget())?;
    let record_b = client.stage_source(
        ingress_b,
        "second.rs".into(),
        b"second source",
        SourceTextAdmission::Excluded,
        &budget(),
    )?;
    check!(record_a != record_b, "distinct source records collapsed");
    let view_b = advance(
        &client,
        ingress_b,
        view_a,
        Some(record_a),
        Some(record_b),
        2,
    )?;
    let ingress_delete = client.begin_input(operation(4), &budget())?;
    let view_deleted = advance(&client, ingress_delete, view_b, Some(record_b), None, 3)?;

    for (view, expected) in [
        (view_a, Some(record_a)),
        (view_b, Some(record_b)),
        (view_deleted, None),
    ] {
        let reply = client.lookup_source_owners(
            ViewSelection::Exact(view),
            vec![owner(1), owner(9)],
            &budget(),
        )?;
        let source = client
            .read_view(ViewSelection::Exact(view), &budget())?
            .source;
        check_lookup(&reply, view, source, expected)?;
    }
    let latest = client.lookup_source_owners(
        ViewSelection::Latest {
            workspace: workspace(),
            epoch: 1,
        },
        vec![owner(1), owner(9)],
        &budget(),
    )?;
    let deleted_source = client
        .read_view(ViewSelection::Exact(view_deleted), &budget())?
        .source;
    check_lookup(&latest, view_deleted, deleted_source, None)?;

    let unknown_view = ViewRef {
        sequence: view_deleted
            .sequence
            .checked_add(1)
            .ok_or("view sequence overflow")?,
        ..view_deleted
    };
    check!(
        matches!(
            client.lookup_source_owners(
                ViewSelection::Exact(unknown_view),
                vec![owner(1)],
                &budget(),
            ),
            Err(NativeUdsError::Refused(error))
                if error.code == NativeIndexErrorCode::SnapshotUnavailable
        ),
        "unknown Exact view did not refuse as unavailable"
    );
    running.stop()?;
    Ok(())
}

#[test]
fn source_owner_lookup_binds_exact_history_and_latest_to_one_source_cut() -> TestResult {
    run_owner_lookup_case()
}
