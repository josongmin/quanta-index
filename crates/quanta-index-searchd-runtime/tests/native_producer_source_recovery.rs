//! Producer baseline recovery through the real Native UDS service and SDK.

#[path = "common/native_physical_profile.rs"]
mod native_physical_profile;

use std::{
    error::Error, os::unix::fs::PermissionsExt, path::Path, sync::Arc, thread::JoinHandle,
    time::Duration,
};

use quanta_index_catalog::SqliteCatalog;
use quanta_index_contract::{
    index::{
        CollectionId, Digest, GroupActivation, ImmutableOperation, IndexInstanceId, IndexKind,
        IndexProfile, IngressRef, InputDomain, InstanceSpec, OperationKey, OperationStatus,
        OwnerKey, OwnerMutation, ProducerId, PublicationGroupId, Refusal, SchemaId, SlotId,
        SourceEvent, SourceInput, SourceTextAdmission, SourceTransition, StableOwnerId,
        StorageFormatId, StreamId, SubmitOutcome, TenantId, UpdateKind, UpdateRequest, ViewRef,
        ViewSelection, WorkspaceKey, WorktreeId,
    },
    native_protocol::{NativeIndexErrorCode, NativeIndexRequest, NativeIndexResponse},
};
use quanta_index_core::{
    RequestBudgetV1,
    index::{
        AdmissionOutcome, IndexError, ObjectInstallScope, RegisteredObject, WorkBudget, WorkLimits,
        effort::ResourceNeed,
        ports::{ImmutableObjectPort, InputStagingPort, ObjectWriteSink, OperationJournalPort},
    },
};
use quanta_index_kernel::AdapterDescriptor;
use quanta_index_sdk::native_uds::{NativeUdsClient, NativeUdsError};
use quanta_index_searchd_runtime::native::{
    NativeAdapterDescriptors, NativeIndexService, NativeResourceObservation, NativeResourcePlan,
    NativeServiceResources,
};
use quanta_index_storage::{FileObjectStore, FileScratchSpace};

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

fn workspace() -> WorkspaceKey {
    WorkspaceKey {
        tenant: TenantId([1; 16]),
        collection: CollectionId([2; 16]),
        worktree: WorktreeId([3; 16]),
        slot: SlotId([4; 16]),
    }
}
fn operation(producer: u8, stream: u8, epoch: u64, sequence: u64) -> OperationKey {
    OperationKey {
        workspace: workspace(),
        epoch: 1,
        producer: ProducerId([producer; 16]),
        stream: StreamId([stream; 16]),
        stream_epoch: epoch,
        sequence,
    }
}
fn owner(producer: u8, id: u8) -> OwnerKey {
    OwnerKey {
        producer: ProducerId([producer; 16]),
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
fn work_budget() -> WorkBudget {
    WorkBudget::with_request(limits(), budget())
}
fn published(outcome: SubmitOutcome) -> TestResult<ViewRef> {
    match outcome {
        SubmitOutcome::Terminal(OperationStatus::Published(view)) => Ok(view),
        other @ (SubmitOutcome::Accepted(_)
        | SubmitOutcome::Terminal(_)
        | SubmitOutcome::Unknown(_)) => Err(format!("expected Published, got {other:?}").into()),
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
            "service leaked an execution reservation"
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

struct IngressSink<'a> {
    catalog: &'a SqliteCatalog,
    ingress: IngressRef,
}
impl ObjectWriteSink for IngressSink<'_> {
    fn managed_scope(&self) -> Option<ObjectInstallScope> {
        Some(ObjectInstallScope::Ingress(self.ingress))
    }
    fn reserve(&self, object: &RegisteredObject) -> Result<(), IndexError> {
        self.catalog
            .reserve_inputs(self.ingress, std::slice::from_ref(object))
    }
    fn installed(&self, object: &RegisteredObject) -> Result<(), IndexError> {
        self.catalog
            .confirm_inputs(self.ingress, std::slice::from_ref(object))
    }
    fn reserve_batch(&self, objects: &[RegisteredObject]) -> Result<(), IndexError> {
        self.catalog.reserve_inputs(self.ingress, objects)
    }
    fn installed_batch(&self, objects: &[RegisteredObject]) -> Result<(), IndexError> {
        self.catalog.confirm_inputs(self.ingress, objects)
    }
}

fn admit_without_driving(root: &Path, ingress: IngressRef, request: &UpdateRequest) -> TestResult {
    let (UpdateKind::SourceAdvance(transition) | UpdateKind::SourceResync { transition, .. }) =
        &request.kind
    else {
        return Err("manual admission needs a source transition".into());
    };
    let catalog = Arc::new(SqliteCatalog::open(root, Duration::from_millis(100))?);
    let scratch_dir = tempfile::tempdir()?;
    let scratch = Arc::new(FileScratchSpace::open_standalone(
        scratch_dir.path().join("scratch"),
        16 * 1024 * 1024 * 1024,
    )?);
    let objects = FileObjectStore::open_managed_with_scratch(
        root.join("native-objects"),
        scratch,
        catalog.clone(),
    )?;
    let serialized = serde_json::to_vec(request)?;
    let stored = objects.install(
        &serialized,
        &transition.owners.object_children(),
        &IngressSink {
            catalog: catalog.as_ref(),
            ingress,
        },
        &mut work_budget(),
    )?;
    catalog.seal_inputs(ingress, stored.reference)?;
    let admission = catalog.admit(
        ImmutableOperation {
            key: request.operation,
            request: stored.reference,
            source_mode: request.kind.source_mode(),
        },
        ingress,
        &transition.events,
        &mut work_budget(),
    )?;
    check!(
        matches!(admission, AdmissionOutcome::New(_)),
        "manual first capture was not admitted"
    );
    check!(
        matches!(
            catalog.inspect(
                request.operation,
                quanta_index_core::index::ports::OperationInspectAccess::Request(&work_budget()),
            )?,
            OperationStatus::Pending(_)
        ),
        "first capture did not remain Pending"
    );
    Ok(())
}

fn create_and_configure(client: &NativeUdsClient) -> TestResult<ViewRef> {
    let reply = client.request(
        NativeIndexRequest::CreateWorkspace {
            workspace: workspace(),
            epoch: 1,
        },
        &budget(),
    )?;
    let NativeIndexResponse::Created(genesis) = reply else {
        return Err(format!("unexpected genesis reply {reply:?}").into());
    };
    let key = operation(1, 1, 1, 1);
    let ingress = client.begin_input(key, &budget())?;
    let profile = client.stage_profile(
        ingress,
        IndexProfile {
            instances: vec![instance(20), instance(30)],
        },
        &budget(),
    )?;
    published(client.submit(
        UpdateRequest {
            operation: key,
            expected_view: genesis,
            kind: UpdateKind::Reconfigure { profile },
        },
        &budget(),
    )?)
}

fn advance(
    client: &NativeUdsClient,
    key: OperationKey,
    base: ViewRef,
    changes: Vec<OwnerMutation>,
    predecessor: u64,
    ordinal: u64,
    prior_epoch: Option<u64>,
) -> TestResult<(ViewRef, UpdateRequest)> {
    let ingress = client.begin_input(key, &budget())?;
    let source = client
        .read_view(ViewSelection::Exact(base), &budget())?
        .source;
    let owners = client.stage_mutations(ingress, changes, &budget())?;
    let mut transition = SourceTransition {
        expected_source: source,
        owners,
        events: Vec::new(),
    };
    transition.events.push(SourceEvent {
        producer: key.producer,
        stream: key.stream,
        stream_epoch: key.stream_epoch,
        predecessor,
        sequence: ordinal,
        payload: transition.owner_digest()?,
    });
    let kind = match prior_epoch {
        Some(prior_epoch) => UpdateKind::SourceResync {
            prior_epoch,
            transition,
        },
        None => UpdateKind::SourceAdvance(transition),
    };
    let request = UpdateRequest {
        operation: key,
        expected_view: base,
        kind,
    };
    let view = published(client.submit(request.clone(), &budget())?)?;
    Ok((view, request))
}

fn pages(
    client: &NativeUdsClient,
    view: ViewRef,
    producer: u8,
    epoch: u64,
) -> TestResult<Vec<quanta_index_contract::native_protocol::NativeProducerSourceOwner>> {
    let mut after = None;
    let mut all = Vec::new();
    loop {
        let page = client.list_producer_source_owners(
            ViewSelection::Exact(view),
            ProducerId([producer; 16]),
            after,
            64,
            &budget(),
        )?;
        check_eq!(page.view, view, "producer page changed exact view");
        check_eq!(
            page.source_epoch,
            Some(epoch),
            "producer page epoch differs"
        );
        let tail = page.rows.as_slice().last().map(|row| row.owner.stable_id);
        all.extend_from_slice(page.rows.as_slice());
        if !page.has_more {
            return Ok(all);
        }
        after = Some(tail.ok_or("continuation page had no rows")?);
    }
}

#[test]
fn producer_page_rebuilds_resync_baseline_over_real_uds() -> TestResult {
    let root = tempfile::tempdir()?;
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))?;
    let socket = root.path().join("native.sock");
    let service = NativeIndexService::bind(
        root.path(),
        &socket,
        limits(),
        NativeAdapterDescriptors {
            literal: AdapterDescriptor {
                kind: instance(20).kind,
                schema: instance(20).schema,
                format: instance(20).format,
            },
            file_map: AdapterDescriptor {
                kind: instance(30).kind,
                schema: instance(30).schema,
                format: instance(30).format,
            },
        },
        resources(),
    )?;
    let running = RunningService::start(service);
    let client = NativeUdsClient::new(socket.clone());
    let configured = create_and_configure(&client)?;
    let initially_empty = client.list_producer_source_owners(
        ViewSelection::Exact(configured),
        ProducerId([5; 16]),
        None,
        64,
        &budget(),
    )?;
    check_eq!(
        initially_empty.source_epoch,
        None,
        "uncaptured producer fabricated epoch"
    );
    check_eq!(
        initially_empty.rows.as_slice().len(),
        0,
        "uncaptured producer fabricated owner"
    );

    let low_ingress = client.begin_input(operation(4, 2, 1, 1), &budget())?;
    let low_ref = client.stage_source(
        low_ingress,
        "outside-low.rs".into(),
        b"low",
        SourceTextAdmission::Excluded,
        &budget(),
    )?;
    let (with_low, _) = advance(
        &client,
        low_ingress.operation,
        configured,
        vec![OwnerMutation {
            owner: owner(4, 1),
            expected: None,
            next: Some(low_ref),
        }],
        0,
        1,
        None,
    )?;
    let target_key = operation(5, 6, 1, 1);
    let target_ingress = client.begin_input(target_key, &budget())?;
    let mut original_refs = Vec::new();
    for start in (1_u8..=129).step_by(32) {
        let end = start.saturating_add(31).min(129);
        let inputs = (start..=end)
            .map(|id| SourceInput {
                path: format!("target/{id:03}.rs"),
                bytes: format!("source {id:03}").into_bytes(),
                text: SourceTextAdmission::Excluded,
            })
            .collect();
        original_refs.extend(client.stage_sources(target_ingress, inputs, &budget())?);
    }
    check_eq!(original_refs.len(), 129, "target staging lost a source");
    let target_mutations: Vec<OwnerMutation> = (1_u8..=129)
        .enumerate()
        .map(|(index, id)| OwnerMutation {
            owner: owner(5, id),
            expected: None,
            next: original_refs.get(index).copied(),
        })
        .collect();
    let source = client
        .read_view(ViewSelection::Exact(with_low), &budget())?
        .source;
    let target_owners = client.stage_mutations(target_ingress, target_mutations, &budget())?;
    let mut target_transition = SourceTransition {
        expected_source: source,
        owners: target_owners,
        events: Vec::new(),
    };
    target_transition.events.push(SourceEvent {
        producer: target_key.producer,
        stream: target_key.stream,
        stream_epoch: 1,
        predecessor: 0,
        sequence: 1,
        payload: target_transition.owner_digest()?,
    });
    let target_request = UpdateRequest {
        operation: target_key,
        expected_view: with_low,
        kind: UpdateKind::SourceAdvance(target_transition),
    };
    // The managed object namespace is exclusive. Handoff the stopped
    // service's custody before constructing the durable Pending fixture.
    running.stop()?;
    admit_without_driving(root.path(), target_ingress, &target_request)?;
    let reopened_service = NativeIndexService::bind(
        root.path(),
        &socket,
        limits(),
        NativeAdapterDescriptors {
            literal: AdapterDescriptor {
                kind: instance(20).kind,
                schema: instance(20).schema,
                format: instance(20).format,
            },
            file_map: AdapterDescriptor {
                kind: instance(30).kind,
                schema: instance(30).schema,
                format: instance(30).format,
            },
        },
        resources(),
    )?;
    let running = RunningService::start(reopened_service);
    let pending_page = client.list_producer_source_owners(
        ViewSelection::Exact(with_low),
        ProducerId([5; 16]),
        None,
        64,
        &budget(),
    )?;
    check_eq!(
        pending_page.source_epoch,
        Some(1),
        "Pending first capture did not expose producer authority"
    );
    check_eq!(
        pending_page.rows.as_slice().len(),
        0,
        "Pending first capture exposed unpublished owners"
    );
    let baseline_target = published(client.resume(target_key, &budget())?)?;
    let high_ingress = client.begin_input(operation(6, 8, 1, 1), &budget())?;
    let high_ref = client.stage_source(
        high_ingress,
        "outside-high.rs".into(),
        b"high",
        SourceTextAdmission::Excluded,
        &budget(),
    )?;
    let (baseline, _) = advance(
        &client,
        high_ingress.operation,
        baseline_target,
        vec![OwnerMutation {
            owner: owner(6, 1),
            expected: None,
            next: Some(high_ref),
        }],
        0,
        1,
        None,
    )?;

    let first = client.list_producer_source_owners(
        ViewSelection::Exact(baseline),
        ProducerId([5; 16]),
        None,
        64,
        &budget(),
    )?;
    check_eq!(
        first.rows.as_slice().len(),
        64,
        "first producer page length differs"
    );
    check!(first.has_more, "first producer page lost continuation");
    let first_after = first
        .rows
        .as_slice()
        .last()
        .ok_or("first page empty")?
        .owner
        .stable_id;
    let first_source = first.source;

    // A source update between pages invalidates the current-owner inventory
    // cursor. Historical exact keyed lookups remain available separately.
    let change_ingress = client.begin_input(operation(5, 6, 1, 2), &budget())?;
    let changed_ref = client.stage_source(
        change_ingress,
        "target/001-new.rs".into(),
        b"changed",
        SourceTextAdmission::Excluded,
        &budget(),
    )?;
    let (changed, change_request) = advance(
        &client,
        change_ingress.operation,
        baseline,
        vec![OwnerMutation {
            owner: owner(5, 1),
            expected: original_refs.first().copied(),
            next: Some(changed_ref),
        }],
        1,
        2,
        None,
    )?;
    check!(
        matches!(
            client.list_producer_source_owners(
                ViewSelection::Exact(baseline),
                ProducerId([5; 16]),
                Some(first_after),
                64,
                &budget(),
            ),
            Err(NativeUdsError::Refused(error)) if error.code == NativeIndexErrorCode::Conflict
        ),
        "historical Exact inventory continued after source changed"
    );
    let selected = [owner(5, 1), owner(5, 65), owner(5, 129)];
    let historical = client.lookup_source_owners(
        ViewSelection::Exact(baseline),
        selected.to_vec(),
        &budget(),
    )?;
    check_eq!(
        historical.source,
        first_source,
        "historical keyed lookup source cut changed"
    );
    for (index, id) in [1_u8, 65, 129].into_iter().enumerate() {
        let row = historical
            .rows
            .as_slice()
            .get(index)
            .ok_or("historical keyed owner absent")?;
        let gold_index = usize::from(id.checked_sub(1).ok_or("invalid gold owner id")?);
        check_eq!(
            row.owner,
            owner(5, id),
            "historical keyed owner order changed"
        );
        check_eq!(
            row.source,
            original_refs.get(gold_index).copied(),
            "historical keyed source ref changed"
        );
    }

    let stale_ingress = client.begin_input(operation(5, 7, 2, 1), &budget())?;
    let stale_owners = client.stage_mutations(
        stale_ingress,
        (1_u8..=129).enumerate().map(|(index, id)| OwnerMutation {
            owner: owner(5, id),
            expected: original_refs.get(index).copied(),
            next: original_refs.get(index).copied(),
        }),
        &budget(),
    )?;
    let mut stale_transition = SourceTransition {
        expected_source: first_source,
        owners: stale_owners,
        events: Vec::new(),
    };
    stale_transition.events.push(SourceEvent {
        producer: stale_ingress.operation.producer,
        stream: stale_ingress.operation.stream,
        stream_epoch: 2,
        predecessor: 0,
        sequence: 1,
        payload: stale_transition.owner_digest()?,
    });
    check!(
        matches!(
            client.submit(
                UpdateRequest {
                    operation: stale_ingress.operation,
                    expected_view: baseline,
                    kind: UpdateKind::SourceResync {
                        prior_epoch: 1,
                        transition: stale_transition
                    },
                },
                &budget()
            ),
            Ok(SubmitOutcome::Terminal(OperationStatus::Refused(
                Refusal::Conflict
            )))
        ),
        "old Exact reset did not fail CAS"
    );

    let current = pages(&client, changed, 5, 1)?;
    check_eq!(current.len(), 129, "current owner prefix length differs");
    for (index, row) in current.iter().enumerate() {
        let id = u8::try_from(index.checked_add(1).ok_or("owner index overflow")?)?;
        let expected = if id == 1 {
            changed_ref
        } else {
            *original_refs
                .get(index)
                .ok_or("current source gold absent")?
        };
        check_eq!(
            row.owner,
            owner(5, id),
            "current owner id differs from fixed gold"
        );
        check_eq!(
            row.source,
            expected,
            "current source ref differs from fixed gold"
        );
    }
    let reset_ingress = client.begin_input(operation(5, 6, 2, 9), &budget())?;
    let reset_owners = client.stage_mutations(
        reset_ingress,
        current.iter().map(|row| OwnerMutation {
            owner: row.owner,
            expected: Some(row.source),
            next: if row.owner == owner(5, 65) {
                None
            } else {
                Some(row.source)
            },
        }),
        &budget(),
    )?;
    let current_source = client
        .read_view(ViewSelection::Exact(changed), &budget())?
        .source;
    let mut reset_transition = SourceTransition {
        expected_source: current_source,
        owners: reset_owners,
        events: Vec::new(),
    };
    reset_transition.events.push(SourceEvent {
        producer: reset_ingress.operation.producer,
        stream: reset_ingress.operation.stream,
        stream_epoch: 2,
        predecessor: 0,
        sequence: 1,
        payload: reset_transition.owner_digest()?,
    });
    let reset_request = UpdateRequest {
        operation: reset_ingress.operation,
        expected_view: changed,
        kind: UpdateKind::SourceResync {
            prior_epoch: 1,
            transition: reset_transition,
        },
    };
    let reset = published(client.submit(reset_request.clone(), &budget())?)?;
    let first_new = client.list_producer_source_owners(
        ViewSelection::Exact(reset),
        ProducerId([5; 16]),
        None,
        64,
        &budget(),
    )?;
    check_eq!(
        first_new.source_epoch,
        Some(2),
        "reset epoch missing from page"
    );
    let reset_rows = client.list_producer_source_owners(
        ViewSelection::Exact(reset),
        ProducerId([5; 16]),
        Some(
            first_new
                .rows
                .as_slice()
                .last()
                .ok_or("reset page empty")?
                .owner
                .stable_id,
        ),
        64,
        &budget(),
    )?;
    check_eq!(
        first_new.rows.as_slice().len(),
        64,
        "reset first page count differs"
    );
    check_eq!(
        reset_rows.rows.as_slice().len(),
        64,
        "reset second page count differs"
    );
    let final_rows = pages(&client, reset, 5, 2)?;
    check_eq!(final_rows.len(), 128, "deleted owner retained after reset");
    check!(
        final_rows.iter().all(|row| row.owner != owner(5, 65)),
        "deleted owner remains in producer page"
    );
    check_eq!(
        final_rows.first().ok_or("reset first row absent")?.source,
        changed_ref,
        "unchanged replaced source lost"
    );
    let low_page = client.list_producer_source_owners(
        ViewSelection::Exact(reset),
        ProducerId([4; 16]),
        None,
        64,
        &budget(),
    )?;
    let high_page = client.list_producer_source_owners(
        ViewSelection::Exact(reset),
        ProducerId([6; 16]),
        None,
        64,
        &budget(),
    )?;
    check_eq!(
        low_page
            .rows
            .as_slice()
            .first()
            .ok_or("low owner missing")?
            .source,
        low_ref,
        "low neighbor changed"
    );
    check_eq!(
        high_page
            .rows
            .as_slice()
            .first()
            .ok_or("high owner missing")?
            .source,
        high_ref,
        "high neighbor changed"
    );
    let stale = client.begin_input(operation(5, 6, 1, 3), &budget())?;
    let stale_owner = client.stage_mutations(
        stale,
        [OwnerMutation {
            owner: owner(5, 2),
            expected: original_refs.get(1).copied(),
            next: original_refs.get(1).copied(),
        }],
        &budget(),
    )?;
    let mut stale_transition = SourceTransition {
        expected_source: client
            .read_view(ViewSelection::Exact(reset), &budget())?
            .source,
        owners: stale_owner,
        events: Vec::new(),
    };
    stale_transition.events.push(SourceEvent {
        producer: stale.operation.producer,
        stream: stale.operation.stream,
        stream_epoch: 1,
        predecessor: 2,
        sequence: 3,
        payload: stale_transition.owner_digest()?,
    });
    check!(
        matches!(
            client.submit(
                UpdateRequest {
                    operation: stale.operation,
                    expected_view: reset,
                    kind: UpdateKind::SourceAdvance(stale_transition)
                },
                &budget()
            ),
            Err(NativeUdsError::Refused(_))
        ),
        "old stream advanced after epoch fence"
    );

    check_eq!(
        client.submit(target_request.clone(), &budget())?,
        SubmitOutcome::Terminal(OperationStatus::Published(baseline_target)),
        "original Published receipt changed"
    );
    check_eq!(
        client.submit(change_request.clone(), &budget())?,
        SubmitOutcome::Terminal(OperationStatus::Published(changed)),
        "inter-page Published receipt changed"
    );
    running.stop()?;
    let restarted = NativeIndexService::bind(
        root.path(),
        &socket,
        limits(),
        NativeAdapterDescriptors {
            literal: AdapterDescriptor {
                kind: instance(20).kind,
                schema: instance(20).schema,
                format: instance(20).format,
            },
            file_map: AdapterDescriptor {
                kind: instance(30).kind,
                schema: instance(30).schema,
                format: instance(30).format,
            },
        },
        resources(),
    )?;
    let running = RunningService::start(restarted);
    let reopened = NativeUdsClient::new(socket);
    check_eq!(
        reopened.submit(reset_request.clone(), &budget())?,
        SubmitOutcome::Terminal(OperationStatus::Published(reset)),
        "reset receipt changed after reopen"
    );
    check_eq!(
        pages(&reopened, reset, 5, 2)?.len(),
        128,
        "current producer prefix changed after reopen"
    );
    let old_lookup = reopened.lookup_source_owners(
        ViewSelection::Exact(baseline),
        vec![owner(5, 65)],
        &budget(),
    )?;
    check_eq!(
        old_lookup
            .rows
            .as_slice()
            .first()
            .ok_or("historical owner absent after reopen")?
            .source,
        original_refs.get(64).copied(),
        "historical keyed source changed after reopen"
    );
    let empty_ingress = reopened.begin_input(operation(5, 6, 3, 11), &budget())?;
    let empty_owners = reopened.stage_mutations(
        empty_ingress,
        final_rows.iter().map(|row| OwnerMutation {
            owner: row.owner,
            expected: Some(row.source),
            next: None,
        }),
        &budget(),
    )?;
    let mut empty_transition = SourceTransition {
        expected_source: reopened
            .read_view(ViewSelection::Exact(reset), &budget())?
            .source,
        owners: empty_owners,
        events: Vec::new(),
    };
    empty_transition.events.push(SourceEvent {
        producer: empty_ingress.operation.producer,
        stream: empty_ingress.operation.stream,
        stream_epoch: 3,
        predecessor: 0,
        sequence: 1,
        payload: empty_transition.owner_digest()?,
    });
    let empty_request = UpdateRequest {
        operation: empty_ingress.operation,
        expected_view: reset,
        kind: UpdateKind::SourceResync {
            prior_epoch: 2,
            transition: empty_transition,
        },
    };
    let empty_view = published(reopened.submit(empty_request.clone(), &budget())?)?;
    let empty_page = reopened.list_producer_source_owners(
        ViewSelection::Exact(empty_view),
        ProducerId([5; 16]),
        None,
        64,
        &budget(),
    )?;
    check_eq!(
        empty_page.source_epoch,
        Some(3),
        "empty reset epoch missing"
    );
    check_eq!(
        empty_page.rows.as_slice().len(),
        0,
        "empty reset retained owners"
    );
    check!(!empty_page.has_more, "empty reset has another page");
    check_eq!(empty_page.next_after(), None, "empty reset has a cursor");
    let empty_paths = reopened.query_paths(
        ViewSelection::Exact(empty_view),
        IndexInstanceId([30; 16]),
        "target/",
        None,
        128,
        &budget(),
    )?;
    check!(
        empty_paths.entries.is_empty(),
        "deleted paths remain searchable"
    );
    let retained_paths = reopened.query_paths(
        ViewSelection::Exact(reset),
        IndexInstanceId([30; 16]),
        "target/",
        None,
        128,
        &budget(),
    )?;
    check_eq!(retained_paths.entries.len(), 128, "old exact paths changed");
    let low_after_empty = pages(&reopened, empty_view, 4, 1)?;
    let high_after_empty = pages(&reopened, empty_view, 6, 1)?;
    check_eq!(low_after_empty.len(), 1, "low neighbor owner count changed");
    check_eq!(
        high_after_empty.len(),
        1,
        "high neighbor owner count changed"
    );
    check_eq!(
        low_after_empty
            .first()
            .ok_or("low neighbor owner missing")?
            .source,
        low_ref,
        "low neighbor ref changed"
    );
    check_eq!(
        high_after_empty
            .first()
            .ok_or("high neighbor owner missing")?
            .source,
        high_ref,
        "high neighbor ref changed"
    );
    check_eq!(
        reopened.submit(target_request, &budget())?,
        SubmitOutcome::Terminal(OperationStatus::Published(baseline_target)),
        "original receipt changed after empty reset"
    );
    check_eq!(
        reopened.submit(change_request, &budget())?,
        SubmitOutcome::Terminal(OperationStatus::Published(changed)),
        "inter-page receipt changed after empty reset"
    );
    check_eq!(
        reopened.submit(reset_request, &budget())?,
        SubmitOutcome::Terminal(OperationStatus::Published(reset)),
        "prior reset receipt changed after empty reset"
    );
    check_eq!(
        reopened.submit(empty_request, &budget())?,
        SubmitOutcome::Terminal(OperationStatus::Published(empty_view)),
        "empty reset receipt changed on replay"
    );
    running.stop()?;
    Ok(())
}
