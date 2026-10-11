//! Opt-in UDS composition proof. The default daemon remains on its existing
//! routes; this exercises the native control socket explicitly.

use std::{
    error::Error,
    os::unix::fs::PermissionsExt,
    os::unix::net::UnixListener,
    sync::{Arc, Condvar, Mutex, mpsc},
    thread::JoinHandle,
    time::{Duration, Instant},
};

use quanta_index_catalog::SqliteCatalog;
use quanta_index_contract::{
    index::{
        CollectionId, Digest, GroupActivation, ImmutableOperation, IndexInstanceId, IndexKind,
        IndexProfile, IngressRef, InputDomain, InstanceSpec, MapRoot, OperationKey,
        OperationStatus, OwnerInputBinding, OwnerKey, OwnerMemberBinding, OwnerMutation,
        ProducerCompleteness, ProducerId, ProjectionCertificate, ProjectionId,
        ProjectionOwnerContribution, ProjectionTransition, PublicationGroupId, SchemaId, SlotId,
        SourceEvent, SourceInput, SourceRecord, SourceTransition, StableOwnerId, StorageFormatId,
        StreamId, SubmitOutcome, TenantId, UpdateKind, UpdateRequest, ViewSelection, WorkIntent,
        WorkScope, WorkspaceKey, WorktreeId,
    },
    native_protocol::{NativeIndexErrorCode, NativeIndexRequest, NativeIndexResponse},
};
use quanta_index_core::{
    RequestBudgetV1,
    index::{
        AdmissionOutcome, AttemptContext, BaseMember, DeltaInput, IndexError, MemberBinding,
        PlanContext, PlanDecision, RegisteredObject, SourceDelta, WorkBudget, WorkLimits,
        ports::{
            ImmutableObjectPort, IndexBuildPort, IndexFuture, IndexOwnerBuildPort,
            InputStagingPort, ObjectWriteSink, OperationJournalPort,
        },
    },
};
use quanta_index_kernel::{AdapterDescriptor, AdapterRegistry};
use quanta_index_lexical::native::{LiteralCandidate, LiteralPlan, NativeLiteralIndex};
use quanta_index_repomap::native::NativeFileMap;
use quanta_index_sdk::native_uds::{NativeUdsClient, NativeUdsError};
use quanta_index_searchd_runtime::native::{
    NativeAdapterDescriptors, NativeIndexService, NativeWorkerObservation, NativeWorkerStatus,
};
use quanta_index_storage::FileObjectStore;
use sha2::{Digest as _, Sha256};

type TestResult<T = ()> = Result<T, Box<dyn Error>>;

fn private_root() -> TestResult<tempfile::TempDir> {
    let root = tempfile::tempdir()?;
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))?;
    Ok(root)
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
        producer: ProducerId([5; 16]),
        stream: StreamId([6; 16]),
        stream_epoch: 1,
        sequence,
    }
}

struct FixtureIngressSink<'a> {
    catalog: &'a SqliteCatalog,
    ingress: IngressRef,
}

impl ObjectWriteSink for FixtureIngressSink<'_> {
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

fn literal_spec() -> InstanceSpec {
    InstanceSpec {
        instance: IndexInstanceId([7; 16]),
        kind: IndexKind([8; 16]),
        group: PublicationGroupId([9; 16]),
        activation: GroupActivation::CoreRequired,
        input_domain: InputDomain::SourceLocal,
        schema: SchemaId([10; 16]),
        policy: Digest([11; 32]),
        format: StorageFormatId([12; 16]),
    }
}

fn file_spec() -> InstanceSpec {
    InstanceSpec {
        instance: IndexInstanceId([17; 16]),
        kind: IndexKind([18; 16]),
        group: PublicationGroupId([9; 16]),
        activation: GroupActivation::CoreRequired,
        input_domain: InputDomain::SourceLocal,
        schema: SchemaId([10; 16]),
        policy: Digest([21; 32]),
        format: StorageFormatId([19; 16]),
    }
}

fn descriptors() -> NativeAdapterDescriptors {
    NativeAdapterDescriptors {
        literal: AdapterDescriptor {
            kind: literal_spec().kind,
            schema: literal_spec().schema,
            format: literal_spec().format,
        },
        file_map: AdapterDescriptor {
            kind: IndexKind([18; 16]),
            schema: SchemaId([10; 16]),
            format: StorageFormatId([19; 16]),
        },
    }
}

struct TokioLiteralIndex;

static LITERAL: NativeLiteralIndex = NativeLiteralIndex;

impl IndexBuildPort for TokioLiteralIndex {
    type Delta = SourceDelta;
    type Plan = LiteralPlan;
    type Candidate = LiteralCandidate;

    fn plan(
        &self,
        base: BaseMember,
        input: DeltaInput<SourceDelta>,
        context: &PlanContext,
        budget: &mut WorkBudget,
    ) -> Result<PlanDecision<LiteralPlan>, IndexError> {
        LITERAL.plan(base, input, context, budget)
    }

    fn prepare<'a>(
        &'a self,
        plan: LiteralPlan,
        context: &'a mut AttemptContext<'_>,
    ) -> IndexFuture<'a, LiteralCandidate> {
        Box::pin(async move {
            // A synchronous futures executor has no Tokio timer reactor. This
            // executes during an actual Kernel SourceAdvance over the UDS.
            tokio::time::sleep(Duration::from_millis(5)).await;
            LITERAL.prepare(plan, context).await
        })
    }

    fn verify_and_seal<'a>(
        &'a self,
        candidate: LiteralCandidate,
        context: &'a mut AttemptContext<'_>,
    ) -> IndexFuture<'a, MemberBinding> {
        LITERAL.verify_and_seal(candidate, context)
    }
}

fn tokio_literal_registry() -> TestResult<AdapterRegistry> {
    let mut registry = AdapterRegistry::new();
    registry.register(descriptors().literal, TokioLiteralIndex, |delta| {
        Ok(delta.clone())
    })?;
    registry.register(descriptors().file_map, NativeFileMap, |delta| {
        Ok(delta.clone())
    })?;
    Ok(registry)
}

fn work_limits() -> WorkLimits {
    WorkLimits {
        reads: 1_000_000,
        writes: 1_000_000,
        bytes: 512 * 1024 * 1024,
        mutations: 1_000_000,
    }
}

fn budget() -> RequestBudgetV1 {
    RequestBudgetV1::until(Instant::now() + Duration::from_secs(30))
}

fn published(outcome: SubmitOutcome) -> TestResult<quanta_index_contract::index::ViewRef> {
    match outcome {
        SubmitOutcome::Terminal(OperationStatus::Published(view)) => Ok(view),
        other => Err(format!("expected published native view, got {other:?}").into()),
    }
}

struct NativeServiceGuard {
    shutdown: quanta_index_ipc::ShutdownHandle,
    thread: Option<JoinHandle<anyhow::Result<()>>>,
    observation: NativeWorkerObservation,
}

impl NativeServiceGuard {
    fn start(service: NativeIndexService) -> Self {
        let shutdown = service.shutdown_handle();
        let observation = service.worker_observation();
        let thread = std::thread::spawn(move || service.run(Duration::from_millis(5)));
        Self {
            shutdown,
            thread: Some(thread),
            observation,
        }
    }

    fn stop(mut self) -> TestResult {
        self.shutdown.trigger();
        if let Some(thread) = self.thread.take() {
            thread
                .join()
                .map_err(|_| "native service thread panicked")??;
        }
        let snapshot = self.observation.observe(Duration::from_secs(1))?;
        assert_eq!(snapshot.status, NativeWorkerStatus::Stopped);
        assert_eq!((snapshot.core.queued, snapshot.core.active), (0, 0));
        assert_eq!(
            (snapshot.enrichment.queued, snapshot.enrichment.active),
            (0, 0)
        );
        assert_eq!(
            snapshot.enrichment.dispatch_lag_ms,
            quanta_index_core::index::effort::Signal::Unknown(
                quanta_index_core::index::effort::UnknownReason::Dropped
            )
        );
        Ok(())
    }
}

impl Drop for NativeServiceGuard {
    fn drop(&mut self) {
        self.shutdown.trigger();
        if let Some(thread) = self.thread.take() {
            drop(thread.join());
        }
    }
}

struct PauseGate {
    entered: mpsc::SyncSender<()>,
    released: Mutex<bool>,
    wake: Condvar,
}

impl PauseGate {
    fn wait(&self) -> Result<(), IndexError> {
        self.entered
            .send(())
            .map_err(|error| IndexError::Storage(format!("owner pause signal: {error}")))?;
        let mut released = self
            .released
            .lock()
            .map_err(|error| IndexError::Storage(format!("owner pause lock: {error}")))?;
        while !*released {
            released = self
                .wake
                .wait(released)
                .map_err(|error| IndexError::Storage(format!("owner pause wait: {error}")))?;
        }
        Ok(())
    }

    fn release(&self) {
        if let Ok(mut released) = self.released.lock() {
            *released = true;
            self.wake.notify_all();
        }
    }
}

struct ReleaseOnDrop(Arc<PauseGate>);

impl Drop for ReleaseOnDrop {
    fn drop(&mut self) {
        self.0.release();
    }
}

struct MaterializeClientGuard {
    gate: Arc<PauseGate>,
    thread: Option<JoinHandle<Result<SubmitOutcome, NativeUdsError>>>,
}

impl MaterializeClientGuard {
    fn finish(mut self) -> TestResult<SubmitOutcome> {
        self.gate.release();
        let thread = self
            .thread
            .take()
            .ok_or("materialize client thread missing")?;
        Ok(thread
            .join()
            .map_err(|_| "materialize client thread panicked")??)
    }
}

impl Drop for MaterializeClientGuard {
    fn drop(&mut self) {
        self.gate.release();
        if let Some(thread) = self.thread.take() {
            drop(thread.join());
        }
    }
}

struct PausedOwnerIndex {
    gate: Arc<PauseGate>,
}

impl IndexOwnerBuildPort for PausedOwnerIndex {
    type Candidate = OwnerMemberBinding;

    fn prepare_owner<'a>(
        &'a self,
        instance: &'a InstanceSpec,
        owner: OwnerKey,
        input: OwnerInputBinding,
        context: &'a mut AttemptContext<'_>,
    ) -> IndexFuture<'a, Self::Candidate> {
        Box::pin(async move {
            context.budget.checkpoint()?;
            self.gate.wait()?;
            context.budget.checkpoint()?;
            let source_ref = input.source.ok_or(IndexError::InvalidInput(
                "paused owner requires a source record".into(),
            ))?;
            let source = context.objects.get(source_ref, context.budget)?;
            let record: SourceRecord = serde_json::from_slice(&source.payload)
                .map_err(|error| IndexError::CorruptObject(format!("owner source: {error}")))?;
            if source.registration.reference != source_ref
                || source.registration.children.as_slice() != [record.bytes]
            {
                return Err(IndexError::CorruptObject(
                    "owner source edges differ".into(),
                ));
            }
            let bytes = context.objects.get(record.bytes, context.budget)?;
            if bytes.registration.reference != record.bytes
                || !bytes.registration.children.is_empty()
                || u64::try_from(bytes.payload.len()).map_err(|_| IndexError::ResourceLimited)?
                    != record.byte_len
            {
                return Err(IndexError::CorruptObject("owner bytes differ".into()));
            }
            let payload = serde_json::to_vec(&(owner, input, record.path, bytes.payload))
                .map_err(|error| IndexError::Storage(format!("owner artifact encode: {error}")))?;
            let artifact = context.put(&payload, &[source_ref])?;
            Ok(OwnerMemberBinding {
                instance: instance.instance,
                owner,
                input,
                format: instance.format,
                artifact,
                logical_commitment: Digest(Sha256::digest(payload).into()),
            })
        })
    }

    fn verify_owner<'a>(
        &'a self,
        candidate: Self::Candidate,
        context: &'a mut AttemptContext<'_>,
    ) -> IndexFuture<'a, OwnerMemberBinding> {
        Box::pin(async move {
            let stored = context.objects.get(candidate.artifact, context.budget)?;
            if stored.registration.reference != candidate.artifact
                || stored.registration.children
                    != candidate.input.source.into_iter().collect::<Vec<_>>()
                || Digest(Sha256::digest(&stored.payload).into()) != candidate.logical_commitment
            {
                return Err(IndexError::CorruptObject(
                    "owner artifact verification failed".into(),
                ));
            }
            Ok(candidate)
        })
    }
}

fn async_owner_spec() -> InstanceSpec {
    InstanceSpec {
        instance: IndexInstanceId([31; 16]),
        kind: IndexKind([32; 16]),
        group: PublicationGroupId([33; 16]),
        activation: GroupActivation::AsyncCoverage,
        input_domain: InputDomain::SourceLocal,
        schema: SchemaId([34; 16]),
        policy: Digest([35; 32]),
        format: StorageFormatId([36; 16]),
    }
}

fn owner(id: u8) -> OwnerKey {
    OwnerKey {
        producer: operation(1).producer,
        stable_id: StableOwnerId([id; 16]),
    }
}

fn stage_source(
    client: &NativeUdsClient,
    operation: OperationKey,
    path: &str,
    bytes: &[u8],
) -> TestResult<(IngressRef, quanta_index_contract::index::ObjectRef)> {
    let ingress = client.begin_input(operation, &budget())?;
    let reference = client.stage_source(ingress, path.into(), bytes, &budget())?;
    Ok((ingress, reference))
}

fn advance_one(
    client: &NativeUdsClient,
    ingress: IngressRef,
    expected_view: quanta_index_contract::index::ViewRef,
    owner: OwnerKey,
    record: quanta_index_contract::index::ObjectRef,
    event_sequence: u64,
) -> TestResult<quanta_index_contract::index::ViewRef> {
    let prior = client.read_view(ViewSelection::Exact(expected_view), &budget())?;
    let owners = client.stage_mutations(
        ingress,
        [OwnerMutation {
            owner,
            expected: None,
            next: Some(record),
        }],
        &budget(),
    )?;
    let mut transition = SourceTransition {
        expected_source: prior.source,
        owners,
        events: Vec::new(),
    };
    transition.events.push(SourceEvent {
        producer: operation(1).producer,
        stream: operation(1).stream,
        stream_epoch: 1,
        predecessor: event_sequence - 1,
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

#[test]
fn native_uds_source_advance_progresses_while_owner_materialization_is_blocked() -> TestResult {
    let root = private_root()?;
    let socket = root.path().join("native.sock");
    let (entered_tx, entered_rx) = mpsc::sync_channel(1);
    let gate = Arc::new(PauseGate {
        entered: entered_tx,
        released: Mutex::new(false),
        wake: Condvar::new(),
    });
    let mut registry = AdapterRegistry::new();
    registry.register(descriptors().literal, NativeLiteralIndex, |delta| {
        Ok(delta.clone())
    })?;
    let owner_spec = async_owner_spec();
    let owner_instance = owner_spec.instance;
    registry.register_owner(
        AdapterDescriptor {
            kind: owner_spec.kind,
            schema: owner_spec.schema,
            format: owner_spec.format,
        },
        PausedOwnerIndex {
            gate: Arc::clone(&gate),
        },
    )?;
    let service =
        NativeIndexService::bind_with_registry(root.path(), &socket, work_limits(), registry)?;
    let service = NativeServiceGuard::start(service);
    // This guard releases the provider before service teardown on every error path.
    let _release_on_drop = ReleaseOnDrop(Arc::clone(&gate));
    let client = NativeUdsClient::new(socket.clone());
    let genesis = match client.request(
        NativeIndexRequest::CreateWorkspace {
            workspace: workspace(),
            epoch: 1,
        },
        &budget(),
    )? {
        NativeIndexResponse::Created(view) => view,
        other => return Err(format!("expected genesis, got {other:?}").into()),
    };
    let profile_ingress = client.begin_input(operation(1), &budget())?;
    let profile = client.stage_profile(
        profile_ingress,
        IndexProfile {
            instances: vec![literal_spec(), owner_spec],
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
    let (ingress_a, source_a) = stage_source(&client, operation(2), "a.rs", b"alpha owner text")?;
    let (ingress_b, source_b) = stage_source(&client, operation(3), "b.rs", b"beta owner text")?;
    let view_a = advance_one(&client, ingress_a, configured, owner(1), source_a, 1)?;
    let ready = match client.request(
        NativeIndexRequest::ReadyWork {
            workspace: workspace(),
            epoch: 1,
            limit: 16,
        },
        &budget(),
    )? {
        NativeIndexResponse::ReadyWork(work) => work,
        other => return Err(format!("expected ready work, got {other:?}").into()),
    };
    let intent: WorkIntent = ready
        .into_iter()
        .find(|candidate| {
            candidate.instance == owner_instance
                && matches!(candidate.scope, WorkScope::Owner { owner: work_owner, .. } if work_owner == owner(1))
        })
        .ok_or("owner A work was not enqueued")?;
    let claim = match client.request(
        NativeIndexRequest::ClaimWork {
            intent,
            lease_millis: 60_000,
        },
        &budget(),
    )? {
        NativeIndexResponse::Claimed(claim) => claim,
        other => return Err(format!("expected work claim, got {other:?}").into()),
    };
    let materialize_key = OperationKey {
        producer: ProducerId([41; 16]),
        stream: StreamId([42; 16]),
        sequence: 1,
        ..operation(1)
    };
    let materialize_socket = socket.clone();
    let materialize = MaterializeClientGuard {
        gate: Arc::clone(&gate),
        thread: Some(std::thread::spawn(move || {
            NativeUdsClient::new(materialize_socket).submit(
                UpdateRequest {
                    operation: materialize_key,
                    expected_view: view_a,
                    kind: UpdateKind::Materialize { claim },
                },
                &budget(),
            )
        })),
    };
    entered_rx
        .recv_timeout(Duration::from_secs(15))
        .map_err(|error| format!("materialize did not enter owner provider: {error}"))?;
    let observed = service.observation.observe(Duration::from_secs(1))?;
    assert_eq!(observed.status, NativeWorkerStatus::Running);
    assert_eq!(
        (observed.enrichment.queued, observed.enrichment.active),
        (0, 1)
    );

    // The owner provider is synchronously blocked on one Tokio worker. The
    // unrelated source update must complete through the same real UDS socket.
    let view_b = advance_one(&client, ingress_b, view_a, owner(2), source_b, 2)?;
    let historical_a = client.read_view(ViewSelection::Exact(view_a), &budget())?;
    let historical_b = client.read_view(ViewSelection::Exact(view_b), &budget())?;
    assert_ne!(historical_a.source, historical_b.source);
    assert_ne!(historical_a.members, historical_b.members);
    assert_eq!(
        client.status(operation(3), &budget())?,
        OperationStatus::Published(view_b)
    );
    let materialized = published(materialize.finish()?)?;
    assert!(materialized.sequence > view_b.sequence);
    let latest = client.read_view(
        ViewSelection::Latest {
            workspace: workspace(),
            epoch: 1,
        },
        &budget(),
    )?;
    assert_eq!(latest.identity, materialized);
    assert_eq!(latest.source, historical_b.source);
    assert_eq!(latest.members, historical_b.members);
    assert_ne!(latest.coverage, historical_b.coverage);
    assert_eq!(
        client.status(materialize_key, &budget())?,
        OperationStatus::Published(materialized)
    );
    assert_eq!(
        client.read_view(ViewSelection::Exact(view_a), &budget())?,
        historical_a
    );
    assert_eq!(
        client.read_view(ViewSelection::Exact(view_b), &budget())?,
        historical_b
    );
    service.stop()
}

#[test]
fn native_uds_source_lifecycle_status_old_view_and_reopen() -> TestResult {
    let root = private_root()?;
    let socket = root.path().join("native.sock");
    let service = NativeIndexService::bind_with_registry(
        root.path(),
        &socket,
        work_limits(),
        tokio_literal_registry()?,
    )?;
    assert!(NativeIndexService::bind(root.path(), &socket, work_limits(), descriptors()).is_err());
    let shutdown = service.shutdown_handle();
    let worker = std::thread::spawn(move || service.run(Duration::from_millis(5)));
    let client = NativeUdsClient::new(socket.clone());
    let genesis = match client.request(
        NativeIndexRequest::CreateWorkspace {
            workspace: workspace(),
            epoch: 1,
        },
        &budget(),
    )? {
        NativeIndexResponse::Created(view) => view,
        other => return Err(format!("unexpected genesis reply: {other:?}").into()),
    };
    let profile_ingress = client.begin_input(operation(1), &budget())?;
    let profile = client.stage_profile(
        profile_ingress,
        IndexProfile {
            instances: vec![literal_spec(), file_spec()],
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
    let changed_profile = client
        .read_view(ViewSelection::Exact(genesis), &budget())?
        .profile;
    assert_ne!(changed_profile, profile);
    assert!(matches!(
        client.submit(
            UpdateRequest {
                operation: operation(1),
                expected_view: genesis,
                kind: UpdateKind::Reconfigure {
                    profile: changed_profile,
                },
            },
            &budget(),
        ),
        Err(NativeUdsError::Refused(error)) if error.code == NativeIndexErrorCode::DigestConflict
    ));
    let source_ingress = client.begin_input(operation(2), &budget())?;
    let staged = client.stage_sources(
        source_ingress,
        vec![
            SourceInput {
                path: "a.rs".into(),
                bytes: b"hello native needle".to_vec(),
            },
            SourceInput {
                path: "b.rs".into(),
                bytes: b"second native needle".to_vec(),
            },
            SourceInput {
                path: "a.rs".into(),
                bytes: b"hello native needle".to_vec(),
            },
        ],
        &budget(),
    )?;
    assert_eq!(staged.len(), 3);
    assert_eq!(staged[0], staged[2]);
    assert_ne!(staged[0], staged[1]);
    let record = staged[0];
    let second_record = staged[1];
    assert!(matches!(
        client.stage_sources(source_ingress, Vec::new(), &budget()),
        Err(NativeUdsError::Protocol(_))
    ));
    let prior = client.read_view(ViewSelection::Exact(configured), &budget())?;
    let owners = client.stage_mutations(
        source_ingress,
        [
            OwnerMutation {
                owner: OwnerKey {
                    producer: ProducerId([5; 16]),
                    stable_id: StableOwnerId([1; 16]),
                },
                expected: None,
                next: Some(record),
            },
            OwnerMutation {
                owner: OwnerKey {
                    producer: ProducerId([5; 16]),
                    stable_id: StableOwnerId([2; 16]),
                },
                expected: None,
                next: Some(second_record),
            },
        ],
        &budget(),
    )?;
    let mut transition = SourceTransition {
        expected_source: prior.source,
        owners,
        events: Vec::new(),
    };
    transition.events.push(SourceEvent {
        producer: ProducerId([5; 16]),
        stream: StreamId([6; 16]),
        stream_epoch: 1,
        predecessor: 0,
        sequence: 1,
        payload: transition.owner_digest()?,
    });
    let next = published(client.submit(
        UpdateRequest {
            operation: operation(2),
            expected_view: configured,
            kind: UpdateKind::SourceAdvance(transition),
        },
        &budget(),
    )?)?;
    assert_eq!(
        client.status(operation(2), &budget())?,
        OperationStatus::Published(next)
    );
    let old = client.read_view(ViewSelection::Exact(configured), &budget())?;
    let current = client.read_view(ViewSelection::Exact(next), &budget())?;
    assert_ne!(old.source.owners, current.source.owners);
    let literal_page = client.search_literal(
        ViewSelection::Exact(next),
        literal_spec().instance,
        b"native",
        None,
        1,
        &budget(),
    )?;
    assert_eq!(literal_page.view, next);
    assert_eq!(literal_page.hits.len(), 1);
    assert_eq!(literal_page.hits[0].record.path, "a.rs");
    assert!(literal_page.has_more);
    let literal_next = client.search_literal(
        ViewSelection::Exact(next),
        literal_spec().instance,
        b"native",
        literal_page.after.as_deref(),
        1,
        &budget(),
    )?;
    assert_eq!(literal_next.hits.len(), 1);
    assert_eq!(literal_next.hits[0].record.path, "b.rs");
    let path_page = client.query_paths(
        ViewSelection::Exact(next),
        file_spec().instance,
        "a",
        None,
        1,
        &budget(),
    )?;
    assert_eq!(path_page.view, next);
    assert_eq!(path_page.entries.len(), 1);
    assert_eq!(path_page.entries[0].path, "a.rs");
    let paths_first = client.query_paths(
        ViewSelection::Exact(next),
        file_spec().instance,
        "",
        None,
        1,
        &budget(),
    )?;
    assert_eq!(paths_first.entries.len(), 1);
    assert!(paths_first.has_more);
    let paths_second = client.query_paths(
        ViewSelection::Exact(next),
        file_spec().instance,
        "",
        Some(&paths_first.entries[0].path),
        1,
        &budget(),
    )?;
    assert_eq!(paths_second.entries.len(), 1);
    assert_eq!(paths_second.entries[0].path, "b.rs");
    assert!(!paths_second.has_more);
    assert!(matches!(
        client.query_paths(
            ViewSelection::Exact(next),
            file_spec().instance,
            "",
            None,
            0,
            &budget(),
        ),
        Err(NativeUdsError::Protocol(_))
    ));
    assert_eq!(
        client
            .search_literal(
                ViewSelection::Exact(configured),
                literal_spec().instance,
                b"native",
                None,
                1,
                &budget(),
            )?
            .hits
            .len(),
        0
    );
    assert_eq!(
        client
            .query_paths(
                ViewSelection::Exact(configured),
                file_spec().instance,
                "a",
                None,
                1,
                &budget(),
            )?
            .entries
            .len(),
        0
    );
    let projection = ProjectionId([20; 16]);
    let projection_producer = ProducerId([15; 16]);
    let projection_operation = OperationKey {
        producer: projection_producer,
        stream: StreamId([7; 16]),
        sequence: 1,
        ..operation(1)
    };
    let projection_ingress = client.begin_input(projection_operation, &budget())?;
    let projection_output =
        client.stage_projection_output(projection_ingress, b"derived", &budget())?;
    let contribution = ProjectionOwnerContribution {
        projection,
        producer: projection_producer,
        owner: OwnerKey {
            producer: projection_producer,
            stable_id: StableOwnerId([1; 16]),
        },
        source_owner: Some(OwnerKey {
            producer: ProducerId([5; 16]),
            stable_id: StableOwnerId([1; 16]),
        }),
        source: Some(record),
        dependency_stamp: Digest([21; 32]),
        output: Some(projection_output),
    };
    let contribution_ref =
        client.stage_projection_owner(projection_ingress, contribution, &budget())?;
    let projection_owners = client.stage_mutations(
        projection_ingress,
        [OwnerMutation {
            owner: contribution.owner,
            expected: None,
            next: Some(contribution_ref),
        }],
        &budget(),
    )?;
    let mut projection_transition = ProjectionTransition {
        projection,
        producer: projection_producer,
        expected: None,
        target_source: current.source,
        schema: SchemaId([22; 16]),
        producer_policy: Digest([23; 32]),
        dependency_context: Digest([24; 32]),
        completeness: ProducerCompleteness::Partial {
            missing: MapRoot(None),
        },
        owners: projection_owners,
        events: Vec::new(),
    };
    let certificate = ProjectionCertificate {
        projection,
        producer: projection_producer,
        prior: None,
        target_source: current.source,
        schema: projection_transition.schema,
        producer_policy: projection_transition.producer_policy,
        dependency_context: projection_transition.dependency_context,
        affected: projection_owners,
    };
    let certificate_ref =
        client.stage_projection_certificate(projection_ingress, certificate, &budget())?;
    projection_transition.completeness = ProducerCompleteness::CompleteAffectedDomain {
        certificate: certificate_ref,
    };
    projection_transition.events.push(SourceEvent {
        producer: projection_operation.producer,
        stream: projection_operation.stream,
        stream_epoch: 1,
        predecessor: 0,
        sequence: 1,
        payload: projection_transition.payload_digest()?,
    });
    let projected = published(client.submit(
        UpdateRequest {
            operation: projection_operation,
            expected_view: next,
            kind: UpdateKind::ProjectionAdvance(Box::new(projection_transition)),
        },
        &budget(),
    )?)?;
    let projected_view = client.read_view(ViewSelection::Exact(projected), &budget())?;
    assert_eq!(projected_view.source, current.source);
    assert_ne!(projected_view.projections, current.projections);
    shutdown.trigger();
    worker
        .join()
        .map_err(|_| "native service thread panicked")??;
    let reopened = NativeIndexService::bind(root.path(), &socket, work_limits(), descriptors())?;
    let stop = reopened.shutdown_handle();
    let worker = std::thread::spawn(move || reopened.run(Duration::from_millis(5)));
    assert_eq!(
        client.status(operation(2), &budget())?,
        OperationStatus::Published(next)
    );
    assert_eq!(
        client.status(projection_operation, &budget())?,
        OperationStatus::Published(projected)
    );
    assert_eq!(
        client.resume(operation(2), &budget())?,
        SubmitOutcome::Terminal(OperationStatus::Published(next))
    );
    assert_eq!(
        client.resume(projection_operation, &budget())?,
        SubmitOutcome::Terminal(OperationStatus::Published(projected))
    );
    let reopened_literal = client.search_literal(
        ViewSelection::Exact(next),
        literal_spec().instance,
        b"native",
        None,
        1,
        &budget(),
    )?;
    assert_eq!(reopened_literal.view, next);
    assert_eq!(reopened_literal.hits[0].record.path, "a.rs");
    let reopened_paths = client.query_paths(
        ViewSelection::Latest {
            workspace: workspace(),
            epoch: 1,
        },
        file_spec().instance,
        "",
        None,
        1,
        &budget(),
    )?;
    assert_eq!(reopened_paths.view, projected);
    assert_eq!(reopened_paths.entries[0].path, "a.rs");
    stop.trigger();
    worker
        .join()
        .map_err(|_| "reopened service thread panicked")??;
    Ok(())
}

#[test]
fn native_uds_resumes_original_pending_request_after_reopen() -> TestResult {
    let root = private_root()?;
    let socket = root.path().join("native.sock");
    let service = NativeIndexService::bind(root.path(), &socket, work_limits(), descriptors())?;
    let shutdown = service.shutdown_handle();
    let worker = std::thread::spawn(move || service.run(Duration::from_millis(5)));
    let client = NativeUdsClient::new(socket.clone());
    let genesis = match client.request(
        NativeIndexRequest::CreateWorkspace {
            workspace: workspace(),
            epoch: 1,
        },
        &budget(),
    )? {
        NativeIndexResponse::Created(view) => view,
        other => return Err(format!("unexpected genesis reply: {other:?}").into()),
    };
    let profile_ingress = client.begin_input(operation(1), &budget())?;
    let profile = client.stage_profile(
        profile_ingress,
        IndexProfile {
            instances: vec![literal_spec()],
        },
        &budget(),
    )?;
    shutdown.trigger();
    worker
        .join()
        .map_err(|_| "native service thread panicked")??;

    // The public durable journal admits an original immutable request before
    // execution. No product test hook or fabricated Pending status is needed.
    let objects = FileObjectStore::open(root.path().join("native-objects"))?;
    let catalog = SqliteCatalog::open(root.path(), Duration::from_secs(2))?;
    let mut work = WorkBudget::with_request(work_limits(), budget());
    let request = UpdateRequest {
        operation: operation(1),
        expected_view: genesis,
        kind: UpdateKind::Reconfigure { profile },
    };
    let request_bytes = serde_json::to_vec(&request)?;
    let sink = FixtureIngressSink {
        catalog: &catalog,
        ingress: profile_ingress,
    };
    let request_object = objects.install(&request_bytes, &[profile], &sink, &mut work)?;
    catalog.seal_inputs(profile_ingress, request_object.reference)?;
    let admitted = catalog.admit(
        ImmutableOperation {
            key: request.operation,
            request: request_object.reference,
        },
        profile_ingress,
        &[],
        &mut work,
    )?;
    assert!(matches!(admitted, AdmissionOutcome::New(_)));
    assert!(matches!(
        catalog.inspect(request.operation)?,
        OperationStatus::Pending(_)
    ));
    drop(catalog);
    drop(objects);

    let reopened = NativeIndexService::bind(root.path(), &socket, work_limits(), descriptors())?;
    let stop = reopened.shutdown_handle();
    let worker = std::thread::spawn(move || reopened.run(Duration::from_millis(5)));
    assert!(matches!(
        client.status(request.operation, &budget())?,
        OperationStatus::Pending(_)
    ));
    let published = published(client.resume(request.operation, &budget())?)?;
    assert_eq!(
        client.status(request.operation, &budget())?,
        OperationStatus::Published(published)
    );
    assert_eq!(
        client.resume(request.operation, &budget())?,
        SubmitOutcome::Terminal(OperationStatus::Published(published))
    );
    let view = client.read_view(ViewSelection::Exact(published), &budget())?;
    assert_eq!(view.profile, profile);
    stop.trigger();
    worker
        .join()
        .map_err(|_| "reopened service thread panicked")??;
    Ok(())
}

#[test]
fn native_format_refuses_unmarked_existing_data_and_symlink_marker() -> TestResult {
    let old = private_root()?;
    std::fs::write(old.path().join("old-data"), b"legacy")?;
    assert!(
        NativeIndexService::bind(
            old.path(),
            &old.path().join("native.sock"),
            work_limits(),
            descriptors(),
        )
        .is_err()
    );
    assert_eq!(std::fs::read(old.path().join("old-data"))?, b"legacy");

    let prior = private_root()?;
    let prior_marker = prior.path().join(".native-index-v1");
    std::fs::write(&prior_marker, b"quanta-index-native-state-v1\n")?;
    std::fs::set_permissions(&prior_marker, std::fs::Permissions::from_mode(0o600))?;
    let prior_error = match NativeIndexService::bind(
        prior.path(),
        &prior.path().join("native.sock"),
        work_limits(),
        descriptors(),
    ) {
        Ok(_service) => return Err("v1 native state unexpectedly reopened".into()),
        Err(error) => error,
    };
    assert!(prior_error.to_string().contains("explicit rebuild"));
    assert!(!prior.path().join(".native-index-v2").exists());

    let symlinked = private_root()?;
    std::fs::write(
        symlinked.path().join("target"),
        b"quanta-index-native-state-v2\n",
    )?;
    std::os::unix::fs::symlink("target", symlinked.path().join(".native-index-v2"))?;
    assert!(
        NativeIndexService::bind(
            symlinked.path(),
            &symlinked.path().join("native.sock"),
            work_limits(),
            descriptors(),
        )
        .is_err()
    );

    let linked = private_root()?;
    let outside = private_root()?;
    let target = outside.path().join("marker-target");
    std::fs::write(&target, b"quanta-index-native-state-v2\n")?;
    std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o600))?;
    std::fs::hard_link(&target, linked.path().join(".native-index-v2"))?;
    assert!(
        NativeIndexService::bind(
            linked.path(),
            &linked.path().join("native.sock"),
            work_limits(),
            descriptors(),
        )
        .is_err()
    );
    Ok(())
}

#[test]
fn native_reopen_accepts_owned_stale_socket_through_root_alias() -> TestResult {
    let root = private_root()?;
    let socket = root.path().join("native.sock");
    let initial = NativeIndexService::bind(root.path(), &socket, work_limits(), descriptors())?;
    drop(initial);

    let stale = UnixListener::bind(&socket)?;
    drop(stale);
    let alias_parent = private_root()?;
    let alias = alias_parent.path().join("root-alias");
    std::os::unix::fs::symlink(root.path(), &alias)?;
    let reopened = NativeIndexService::bind(
        &alias,
        &alias.join("native.sock"),
        work_limits(),
        descriptors(),
    )?;
    drop(reopened);
    Ok(())
}

#[test]
fn native_bind_refuses_nested_socket_before_format_creation() -> TestResult {
    let root = private_root()?;
    let alias_parent = private_root()?;
    let alias = alias_parent.path().join("root-alias");
    std::os::unix::fs::symlink(root.path(), &alias)?;
    for socket in [
        root.path().join("run/native.sock"),
        alias.join("run/not-created/native.sock"),
    ] {
        let error =
            match NativeIndexService::bind(root.path(), &socket, work_limits(), descriptors()) {
                Ok(service) => {
                    drop(service);
                    return Err("nested socket unexpectedly accepted".into());
                }
                Err(error) => error,
            };
        assert!(error.to_string().contains("nested directory"));
        assert!(!root.path().join(".native-index-v2").exists());
        assert!(!root.path().join("catalog").exists());
        assert!(!root.path().join("native-objects").exists());
        assert!(!root.path().join("run").exists());
    }
    let traversal = root.path().join("missing/../native.sock");
    let error =
        match NativeIndexService::bind(root.path(), &traversal, work_limits(), descriptors()) {
            Ok(service) => {
                drop(service);
                return Err("unresolved parent traversal unexpectedly accepted".into());
            }
            Err(error) => error,
        };
    assert!(error.to_string().contains("unresolved parent traversal"));
    assert!(!root.path().join(".native-index-v2").exists());
    assert!(!root.path().join("missing").exists());
    Ok(())
}
