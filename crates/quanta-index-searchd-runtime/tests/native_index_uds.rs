//! Opt-in UDS composition proof. The default daemon remains on its existing
//! routes; this exercises the native control socket explicitly.
#![expect(
    clippy::panic_in_result_fn,
    reason = "integration test assertions are the failure oracle"
)]

#[path = "common/native_physical_profile.rs"]
mod native_physical_profile;

#[path = "common/native_object_files.rs"]
mod native_object_files;

#[path = "native_index_uds/physical_mixed_pressure.rs"]
mod physical_mixed_pressure;

#[path = "native_index_uds/read_sessions.rs"]
mod read_sessions;

use std::fmt::Write as _;
use std::{
    error::Error,
    os::unix::fs::PermissionsExt,
    os::unix::net::UnixListener,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

use quanta_index_catalog::SqliteCatalog;
use quanta_index_contract::{
    EmbeddingNormalization, QueryConstraintSetV1, SemanticQueryRequest, TextQueryRequest,
    TextQuerySyntax,
    index::{
        CollectionId, Digest, GroupActivation, ImmutableOperation, IndexInstanceId, IndexKind,
        IndexProfile, IngressRef, InputDomain, InstanceSpec, MapRoot, OperationKey,
        OperationStatus, OwnerInputBinding, OwnerKey, OwnerMemberBinding, OwnerMutation,
        ProducerCompleteness, ProducerId, ProjectionBinding, ProjectionCertificate, ProjectionId,
        ProjectionOwnerContribution, ProjectionTransition, PublicationGroupId, SchemaId, SlotId,
        SourceEvent, SourceInput, SourceRecord, SourceTextAdmission, SourceTransition,
        StableOwnerId, StorageFormatId, StreamId, SubmitOutcome, TenantId, UpdateKind,
        UpdateRequest, ViewSelection, WorkClaim, WorkIntent, WorkScope, WorkStatus, WorkspaceKey,
        WorktreeId,
    },
    native_protocol::{
        NativeIndexErrorCode, NativeIndexRequest, NativeIndexResponse, NativeSemanticChunkSpan,
        NativeSemanticManifestHeader, NativeSemanticManifestMember, NativeTextCoverage,
    },
};
use quanta_index_core::{
    CoreError, RequestBudgetV1, TextEmbeddingProvider,
    index::{
        AdmissionOutcome, AttemptContext, BaseMember, DeltaInput, IndexError, IndexQueryPort,
        MemberBinding, ObjectInstallScope, ObjectRef, PlanContext, PlanDecision, QueryOutcome,
        QueryReadContext, RegisteredObject, SourceDelta, WorkBudget, WorkLimits,
        effort::ResourceNeed,
        ports::{
            CatalogGenesisPort, CatalogReadPort, CatalogReclaimPort, ImmutableObjectPort,
            IndexBuildPort, IndexFuture, IndexOwnerBuildPort, InputStagingPort, ObjectWriteSink,
            OperationJournalPort,
        },
    },
};
use quanta_index_kernel::{AdapterDescriptor, AdapterRegistry, QueryAdapterRegistry};
use quanta_index_lexical::native::{
    Bm25Policy, LiteralCandidate, LiteralPage, LiteralPlan, LiteralReadRequest, NativeLiteralIndex,
    NativeLiteralReader, NativeTextIndex, NativeTextReader, PlannedTextRead, TextReadResult,
    TextRecipe,
};
use quanta_index_repomap::native::{NativeFileMap, NativePathReader};
use quanta_index_sdk::native_uds::{NativeUdsClient, NativeUdsError};
use quanta_index_searchd_runtime::native::{
    FixedPumpPolicy, NativeAdapterDescriptors, NativeIndexService, NativePumpConfig,
    NativePumpScope, NativeReclaimConfig, NativeResourceObservation, NativeResourcePlan,
    NativeServiceCapabilities, NativeServiceResources, NativeWorkerObservation, NativeWorkerStatus,
};
use quanta_index_semantic::native::{
    NativeSemantic, NativeSemanticReadConfig, NativeSemanticReader, SearchResult,
    TextEmbeddingBridge, encode_certified_manifest,
};
use quanta_index_storage::{FileObjectStore, FileScratchSpace};
use sha2::{Digest as _, Sha256};

type TestResult<T = ()> = Result<T, Box<dyn Error>>;

struct ForcedExitChild(std::process::Child);

impl Drop for ForcedExitChild {
    fn drop(&mut self) {
        drop(self.0.kill());
        drop(self.0.wait());
    }
}

fn test_resources() -> NativeServiceResources {
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
                // Four execution floors plus one full-stage escaping-data allowance.
                memory_bytes: (4 + 1) * 64 * 1024 * 1024,
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

fn bind_service(
    root: &std::path::Path,
    socket: &std::path::Path,
    limits: WorkLimits,
    descriptors: NativeAdapterDescriptors,
) -> anyhow::Result<NativeIndexService> {
    NativeIndexService::bind(root, socket, limits, descriptors, test_resources())
}

fn bind_service_with_registry(
    root: &std::path::Path,
    socket: &std::path::Path,
    limits: WorkLimits,
    registry: AdapterRegistry,
) -> anyhow::Result<NativeIndexService> {
    let resources = test_resources();
    let reader_need = ResourceNeed {
        provider_units: 0,
        ..resources.execution.interactive
    };
    let mut literal = QueryAdapterRegistry::new();
    literal.register(
        descriptors().literal,
        reader_need,
        NativeLiteralReader::new(reader_need)?,
    )?;
    let mut path = QueryAdapterRegistry::new();
    path.register(
        descriptors().file_map,
        reader_need,
        NativePathReader::new(reader_need)?,
    )?;
    NativeIndexService::bind_with_capabilities(
        root,
        socket,
        limits,
        NativeServiceCapabilities {
            builders: registry,
            literal,
            path,
            text: QueryAdapterRegistry::new(),
            semantic: QueryAdapterRegistry::new(),
            pump: None,
            provider_attempt_pool: None,
        },
        resources,
    )
}

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

    fn resource_demand(&self) -> ResourceNeed {
        ResourceNeed::default()
    }

    fn validate_instance(
        &self,
        instance: &quanta_index_core::index::InstanceSpec,
    ) -> Result<(), IndexError> {
        NativeLiteralIndex.validate_instance(instance)
    }

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

fn test_reclaim_config() -> NativeReclaimConfig {
    NativeReclaimConfig {
        interval: Duration::from_millis(20),
        max_steps_per_round: 1,
        child_limit: 32,
        work_limits: work_limits(),
        round_timeout: Duration::from_secs(2),
        head_probe_every: 4,
    }
}

fn reclaim_capabilities(pump: Option<NativePumpConfig>) -> TestResult<NativeServiceCapabilities> {
    Ok(NativeServiceCapabilities {
        builders: tokio_literal_registry()?,
        literal: QueryAdapterRegistry::new(),
        path: QueryAdapterRegistry::new(),
        text: QueryAdapterRegistry::new(),
        semantic: QueryAdapterRegistry::new(),
        pump,
        provider_attempt_pool: None,
    })
}

#[test]
fn native_reclaim_step_refuses_insufficient_control_envelope_without_reservation() -> TestResult {
    let root = private_root()?;
    let socket = root.path().join("native.sock");
    let mut resources = test_resources();
    resources.execution.control.memory_bytes = 4 * 1024 * 1024;
    let service = NativeIndexService::bind(
        root.path(),
        &socket,
        work_limits(),
        descriptors(),
        resources,
    )?;
    let maintenance = service.maintenance_handle();
    let observation = service.resource_observation();
    let running = NativeServiceGuard::start(service);
    let client = NativeUdsClient::new(socket);
    let reply = client.request(
        NativeIndexRequest::CreateWorkspace {
            workspace: workspace(),
            epoch: 1,
        },
        &budget(),
    )?;
    if !matches!(reply, NativeIndexResponse::Created(_)) {
        return Err(format!("genesis failed before GC admission fixture: {reply:?}").into());
    }
    let (ingress, record) = stage_source(&client, operation(600), "denied.rs", b"GC denied")?;
    let record_file = native_object_file(root.path(), record)?;
    client.discard_input(ingress, &budget())?;
    assert!(record_file.is_file());
    let catalog = SqliteCatalog::open(root.path(), Duration::from_secs(2))?;
    let queued_before = catalog.reclaim_work_page(
        None,
        8,
        &mut WorkBudget::with_request(work_limits(), budget()),
    )?;
    assert!(
        queued_before
            .items
            .iter()
            .any(|item| item.object() == record)
    );
    let before = observation.observe()?;
    let result = maintenance.reclaim_step(
        None,
        1,
        &mut WorkBudget::with_request(work_limits(), budget()),
    );
    assert!(matches!(result, Err(IndexError::ResourceLimited)));
    assert_eq!(observation.observe()?, before);
    assert!(record_file.is_file());
    let queued_after = catalog.reclaim_work_page(
        None,
        8,
        &mut WorkBudget::with_request(work_limits(), budget()),
    )?;
    assert_eq!(queued_after, queued_before);
    running.stop()
}

#[test]
fn native_reclaim_step_uses_caller_cancellation_before_reservation() -> TestResult {
    let root = private_root()?;
    let service = bind_service(
        root.path(),
        &root.path().join("native.sock"),
        work_limits(),
        descriptors(),
    )?;
    let observation = service.resource_observation();
    let before = observation.observe()?;
    let request = budget();
    request.cancel_handle().cancel();
    let result = service.maintenance_handle().reclaim_step(
        None,
        1,
        &mut WorkBudget::with_request(work_limits(), request),
    );
    assert!(matches!(result, Err(IndexError::Cancelled)));
    assert_eq!(observation.observe()?, before);
    Ok(())
}

#[test]
fn native_maintenance_explicit_bootstrap_abort_fences_old_stage() -> TestResult {
    let root = private_root()?;
    let identity_root = private_root()?;
    let identity = FileObjectStore::open(identity_root.path().join("identity-objects"))?;
    let bind = || {
        bind_service(
            root.path(),
            &root.path().join("native.sock"),
            work_limits(),
            descriptors(),
        )
    };
    // Seed durable stages between service lifetimes. A raw Catalog mutation
    // must not borrow an active service's managed physical namespace.
    drop(bind()?);
    let catalog = SqliteCatalog::open(root.path(), Duration::from_secs(2))?;
    let stage = catalog.begin_bootstrap(
        workspace(),
        1,
        &identity,
        &mut WorkBudget::with_request(work_limits(), budget()),
    )?;
    let catalog_path = catalog.path().to_path_buf();
    let connection = rusqlite::Connection::open_with_flags(
        &catalog_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?;
    let before: i64 =
        connection.query_row("SELECT count(*) FROM native_bootstrap_v1", [], |row| {
            row.get(0)
        })?;
    assert_eq!(before, 1);
    drop(connection);
    drop(catalog);
    let service = bind()?;
    service.maintenance_handle().abort_bootstrap(
        stage,
        &mut WorkBudget::with_request(work_limits(), budget()),
    )?;
    let connection = rusqlite::Connection::open_with_flags(
        &catalog_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?;
    let after: i64 =
        connection.query_row("SELECT count(*) FROM native_bootstrap_v1", [], |row| {
            row.get(0)
        })?;
    assert_eq!(after, 0);
    drop(connection);
    drop(service);
    let catalog = SqliteCatalog::open(root.path(), Duration::from_secs(2))?;
    let replacement = catalog.begin_bootstrap(
        workspace(),
        1,
        &identity,
        &mut WorkBudget::with_request(work_limits(), budget()),
    )?;
    assert_eq!(replacement.profile, stage.profile);
    assert!(replacement.fence > stage.fence);
    drop(catalog);
    let service = bind()?;
    assert!(matches!(
        service.maintenance_handle().abort_bootstrap(
            stage,
            &mut WorkBudget::with_request(work_limits(), budget())
        ),
        Err(IndexError::FenceLost)
    ));
    Ok(())
}

fn native_object_file(
    root: &std::path::Path,
    reference: ObjectRef,
) -> TestResult<std::path::PathBuf> {
    Ok(native_object_files::object_file(
        &root.join("native-objects"),
        reference,
    )?)
}

fn native_object_absent_nofollow(path: &std::path::Path) -> std::io::Result<bool> {
    native_object_files::absent_nofollow(path)
}

fn native_objects_absent_nofollow(
    root: &std::path::Path,
    objects: &[ObjectRef],
) -> TestResult<bool> {
    let mut all_absent = true;
    for object in objects {
        all_absent &= native_object_absent_nofollow(&native_object_file(root, *object)?)?;
    }
    Ok(all_absent)
}

#[test]
fn native_reclaim_step_physically_removes_discarded_source_record_and_body() -> TestResult {
    let root = private_root()?;
    let identity_root = private_root()?;
    let identity = FileObjectStore::open(identity_root.path().join("identity-objects"))?;
    let socket = root.path().join("native.sock");
    let service = bind_service(root.path(), &socket, work_limits(), descriptors())?;
    let maintenance = service.maintenance_handle();
    let observation = service.resource_observation();
    let running = NativeServiceGuard::start(service);
    let client = NativeUdsClient::new(socket);
    let reply = client.request(
        NativeIndexRequest::CreateWorkspace {
            workspace: workspace(),
            epoch: 1,
        },
        &budget(),
    )?;
    if !matches!(reply, NativeIndexResponse::Created(_)) {
        return Err(format!("genesis failed before GC fixture: {reply:?}").into());
    }
    let body = b"runtime GC source body";
    let (ingress, record) = stage_source(&client, operation(601), "gc.rs", body)?;
    let content = identity
        .identify(
            body,
            &[],
            &mut WorkBudget::with_request(work_limits(), budget()),
        )?
        .reference;
    let record_file = native_object_file(root.path(), record)?;
    let content_file = native_object_file(root.path(), content)?;
    assert!(record_file.is_file());
    assert!(content_file.is_file());
    client.discard_input(ingress, &budget())?;

    let constrained = WorkLimits {
        bytes: 0,
        ..work_limits()
    };
    let denied = maintenance.reclaim_step(
        None,
        1,
        &mut WorkBudget::with_request(constrained, budget()),
    );
    assert!(matches!(denied, Err(IndexError::ResourceLimited)));
    assert!(record_file.is_file());
    assert!(content_file.is_file());
    assert_eq!(observation.observe()?.reserved, ResourceNeed::default());

    let mut after = None;
    let mut examined = Vec::new();
    for _step in 0..8 {
        let step = maintenance.reclaim_step(
            after,
            1,
            &mut WorkBudget::with_request(work_limits(), budget()),
        )?;
        if let Some(object) = step.examined {
            examined.push(object);
            if !step.drain.is_some_and(|page| page.has_more) {
                assert_eq!(step.resume_after, Some(object));
            }
        }
        assert_eq!(observation.observe()?.reserved, ResourceNeed::default());
        if !record_file.exists() && !content_file.exists() {
            break;
        }
        after = if step.has_more {
            step.resume_after
        } else {
            None
        };
    }
    assert!(
        !record_file.exists(),
        "discarded record survived bounded GC steps"
    );
    assert!(
        !content_file.exists(),
        "discarded body survived bounded GC steps"
    );
    assert!(examined.contains(&record));
    assert!(examined.contains(&content));
    let catalog = SqliteCatalog::open(root.path(), Duration::from_secs(2))?;
    let queue = catalog.reclaim_work_page(
        None,
        8,
        &mut WorkBudget::with_request(work_limits(), budget()),
    )?;
    assert!(
        !queue
            .items
            .iter()
            .any(|item| { item.object() == record || item.object() == content })
    );
    running.stop()
}

#[test]
fn native_reclaim_resumes_retiring_body_after_budget_failure_and_reopen() -> TestResult {
    let root = private_root()?;
    let identity_root = private_root()?;
    let identity = FileObjectStore::open(identity_root.path().join("identity-objects"))?;
    let socket = root.path().join("native.sock");
    let service = bind_service(root.path(), &socket, work_limits(), descriptors())?;
    let running = NativeServiceGuard::start(service);
    let client = NativeUdsClient::new(socket.clone());
    let created = client.request(
        NativeIndexRequest::CreateWorkspace {
            workspace: workspace(),
            epoch: 1,
        },
        &budget(),
    )?;
    if !matches!(created, NativeIndexResponse::Created(_)) {
        return Err(format!("genesis failed before GC restart fixture: {created:?}").into());
    }
    let body = vec![b'G'; 1024 * 1024];
    let (ingress, record) = stage_source(&client, operation(602), "gc-resume.rs", &body)?;
    let content = identity
        .identify(
            &body,
            &[],
            &mut WorkBudget::with_request(work_limits(), budget()),
        )?
        .reference;
    let record_file = native_object_file(root.path(), record)?;
    let content_file = native_object_file(root.path(), content)?;
    assert!(record_file.is_file());
    assert!(content_file.is_file());
    client.discard_input(ingress, &budget())?;
    running.stop()?;
    // Raw fixture initialization writes only while the admitted service is
    // stopped. Keep this connection for read-only queue observation afterwards.
    let catalog = SqliteCatalog::open(root.path(), Duration::from_secs(2))?;
    let service = bind_service(root.path(), &socket, work_limits(), descriptors())?;
    let maintenance = service.maintenance_handle();
    let observation = service.resource_observation();
    let running = NativeServiceGuard::start(service);
    let queued = catalog.reclaim_work_page(
        None,
        8,
        &mut WorkBudget::with_request(work_limits(), budget()),
    )?;
    let record_position = queued
        .items
        .iter()
        .position(|work| work.object() == record)
        .ok_or("discarded source record was not queued")?;
    let record_after = record_position
        .checked_sub(1)
        .map(|index| {
            queued
                .items
                .get(index)
                .map(quanta_index_core::index::ReclaimWork::object)
                .ok_or("record cursor disappeared from queued page")
        })
        .transpose()?;
    let first = maintenance.reclaim_step(
        record_after,
        1,
        &mut WorkBudget::with_request(work_limits(), budget()),
    )?;
    assert_eq!(first.examined, Some(record));
    assert!(!record_file.exists());
    assert!(content_file.is_file());

    let queued = catalog.reclaim_work_page(
        None,
        8,
        &mut WorkBudget::with_request(work_limits(), budget()),
    )?;
    let content_position = queued
        .items
        .iter()
        .position(|work| work.object() == content)
        .ok_or("discarded source body was not queued after parent drain")?;
    let content_after = content_position
        .checked_sub(1)
        .map(|index| {
            queued
                .items
                .get(index)
                .map(quanta_index_core::index::ReclaimWork::object)
                .ok_or("content cursor disappeared from queued page")
        })
        .transpose()?;
    let wire_bytes = content_file.metadata()?.len();
    let limited_bytes = wire_bytes
        .checked_div(2)
        .filter(|half| *half > 0)
        .ok_or("content object must have at least two encoded bytes")?;
    let limited = WorkLimits {
        bytes: limited_bytes,
        ..work_limits()
    };
    let mut original_budget = WorkBudget::with_request(limited, budget());
    let denied = maintenance.reclaim_step(content_after, 1, &mut original_budget);
    assert!(matches!(denied, Err(IndexError::ResourceLimited)));
    assert!(
        original_budget.usage().writes > 0,
        "the budget must admit the durable Retiring transition before stream refusal"
    );
    assert!(content_file.is_file());
    let phase: i64 = rusqlite::Connection::open_with_flags(
        catalog.path(),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?
    .query_row(
        "SELECT phase FROM native_gc_object_v1 WHERE object=?1",
        rusqlite::params![(content.0).0.to_vec()],
        |row| row.get(0),
    )?;
    assert_eq!(phase, 1, "the failed stream must leave Retiring durable");
    assert_eq!(observation.observe()?.reserved, ResourceNeed::default());

    drop(catalog);
    drop(maintenance);
    running.stop()?;
    let reopened = bind_service(root.path(), &socket, work_limits(), descriptors())?;
    let reopened_maintenance = reopened.maintenance_handle();
    let resumed = reopened_maintenance.reclaim_step(
        content_after,
        1,
        &mut WorkBudget::with_request(work_limits(), budget()),
    )?;
    assert_eq!(resumed.examined, Some(content));
    assert!(!content_file.exists());
    assert_eq!(
        reopened.resource_observation().observe()?.reserved,
        ResourceNeed::default()
    );
    Ok(())
}

#[test]
fn native_uds_unregistered_literal_and_path_readers_refuse_selected_profile() -> TestResult {
    let root = private_root()?;
    let socket = root.path().join("native.sock");
    let service = NativeIndexService::bind_with_registry(
        root.path(),
        &socket,
        work_limits(),
        tokio_literal_registry()?,
        test_resources(),
    )?;
    let observation = service.resource_observation();
    let running = NativeServiceGuard::start(service);
    let client = NativeUdsClient::new(socket);
    let created = client.request(
        NativeIndexRequest::CreateWorkspace {
            workspace: workspace(),
            epoch: 1,
        },
        &budget(),
    )?;
    let NativeIndexResponse::Created(genesis) = created else {
        return Err(format!("unexpected genesis reply: {created:?}").into());
    };
    let ingress = client.begin_input(operation(603), &budget())?;
    let profile = client.stage_profile(
        ingress,
        IndexProfile {
            instances: vec![literal_spec(), file_spec()],
        },
        &budget(),
    )?;
    let configured = published(&client.submit(
        UpdateRequest {
            operation: operation(603),
            expected_view: genesis,
            kind: UpdateKind::Reconfigure { profile },
        },
        &budget(),
    )?)?;
    assert!(matches!(
        client.search_literal(
            ViewSelection::Exact(configured),
            literal_spec().instance,
            b"needle",
            None,
            1,
            &budget(),
        ),
        Err(NativeUdsError::Refused(error))
            if error.code == NativeIndexErrorCode::SnapshotUnavailable
    ));
    assert!(matches!(
        client.query_paths(
            ViewSelection::Exact(configured),
            file_spec().instance,
            "",
            None,
            1,
            &budget(),
        ),
        Err(NativeUdsError::Refused(error))
            if error.code == NativeIndexErrorCode::SnapshotUnavailable
    ));
    assert_eq!(observation.observe()?.reserved, ResourceNeed::default());
    let catalog = SqliteCatalog::open(root.path(), Duration::from_secs(2))?;
    let pins: i64 = rusqlite::Connection::open(catalog.path())?.query_row(
        "SELECT COUNT(*) FROM native_pin_v1",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(pins, 0);
    running.stop()
}

struct RecordingLiteralReader {
    need: ResourceNeed,
    selected: Arc<Mutex<Vec<(IndexInstanceId, Digest)>>>,
}

impl IndexQueryPort<LiteralReadRequest, LiteralPage> for RecordingLiteralReader {
    fn validate_instance(&self, instance: &InstanceSpec) -> Result<(), IndexError> {
        if instance.kind != literal_spec().kind {
            return Err(IndexError::InvalidInput("unexpected literal kind".into()));
        }
        Ok(())
    }

    fn reservation(&self, request: &LiteralReadRequest) -> Result<ResourceNeed, IndexError> {
        request.validate()?;
        Ok(self.need)
    }

    fn query(
        &self,
        _request: &LiteralReadRequest,
        context: &mut QueryReadContext<'_>,
    ) -> Result<QueryOutcome<LiteralPage>, IndexError> {
        self.selected
            .lock()
            .map_err(|error| IndexError::BackendViolation(error.to_string()))?
            .push((context.instance.instance, context.instance.policy));
        QueryOutcome::core_scanned(
            context.ticket,
            context.instance,
            true,
            LiteralPage::from_parts(Vec::new(), None, false, None, None),
        )
    }
}

#[test]
fn native_uds_same_kind_instances_resolve_separate_stored_specs() -> TestResult {
    let root = private_root()?;
    let socket = root.path().join("native.sock");
    let resources = test_resources();
    let need = ResourceNeed {
        provider_units: 0,
        ..resources.execution.interactive
    };
    let selected = Arc::new(Mutex::new(Vec::new()));
    let mut literal = QueryAdapterRegistry::new();
    literal.register(
        descriptors().literal,
        need,
        RecordingLiteralReader {
            need,
            selected: Arc::clone(&selected),
        },
    )?;
    let service = NativeIndexService::bind_with_capabilities(
        root.path(),
        &socket,
        work_limits(),
        NativeServiceCapabilities {
            builders: tokio_literal_registry()?,
            literal,
            path: QueryAdapterRegistry::new(),
            text: QueryAdapterRegistry::new(),
            semantic: QueryAdapterRegistry::new(),
            pump: None,
            provider_attempt_pool: None,
        },
        resources,
    )?;
    let observation = service.resource_observation();
    let running = NativeServiceGuard::start(service);
    let client = NativeUdsClient::new(socket);
    let created = client.request(
        NativeIndexRequest::CreateWorkspace {
            workspace: workspace(),
            epoch: 1,
        },
        &budget(),
    )?;
    let NativeIndexResponse::Created(genesis) = created else {
        return Err(format!("unexpected genesis reply: {created:?}").into());
    };
    let first = literal_spec();
    let mut second = literal_spec();
    second.instance = IndexInstanceId([91; 16]);
    second.group = PublicationGroupId([92; 16]);
    second.policy = Digest([93; 32]);
    let ingress = client.begin_input(operation(605), &budget())?;
    let profile = client.stage_profile(
        ingress,
        IndexProfile {
            instances: vec![first.clone(), second.clone()],
        },
        &budget(),
    )?;
    let configured = published(&client.submit(
        UpdateRequest {
            operation: operation(605),
            expected_view: genesis,
            kind: UpdateKind::Reconfigure { profile },
        },
        &budget(),
    )?)?;
    for spec in [&first, &second] {
        let page = client.search_literal(
            ViewSelection::Exact(configured),
            spec.instance,
            b"needle",
            None,
            1,
            &budget(),
        )?;
        assert_eq!(page.view, configured);
        assert!(page.hits.is_empty());
        assert!(!page.has_more);
    }
    assert_eq!(
        *selected.lock().map_err(|error| error.to_string())?,
        vec![
            (first.instance, first.policy),
            (second.instance, second.policy)
        ]
    );
    assert_eq!(observation.observe()?.reserved, ResourceNeed::default());
    let catalog = SqliteCatalog::open(root.path(), Duration::from_secs(2))?;
    let pins: i64 = rusqlite::Connection::open(catalog.path())?.query_row(
        "SELECT COUNT(*) FROM native_pin_v1",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(pins, 0);
    running.stop()
}

#[test]
fn native_uds_same_kind_real_members_bind_policy_and_preserve_old_view() -> TestResult {
    let root = private_root()?;
    let socket = root.path().join("native.sock");
    let service = bind_service_with_registry(
        root.path(),
        &socket,
        work_limits(),
        tokio_literal_registry()?,
    )?;
    let running = NativeServiceGuard::start(service);
    let client = NativeUdsClient::new(socket);
    let NativeIndexResponse::Created(genesis) = client.request(
        NativeIndexRequest::CreateWorkspace {
            workspace: workspace(),
            epoch: 1,
        },
        &budget(),
    )?
    else {
        return Err("same-kind genesis reply differs".into());
    };
    let first = literal_spec();
    let mut second = literal_spec();
    second.instance = IndexInstanceId([91; 16]);
    second.group = PublicationGroupId([92; 16]);
    second.policy = Digest([93; 32]);
    let ingress = client.begin_input(operation(606), &budget())?;
    let profile = client.stage_profile(
        ingress,
        IndexProfile {
            instances: vec![first.clone(), second.clone()],
        },
        &budget(),
    )?;
    let configured = published(&client.submit(
        UpdateRequest {
            operation: operation(606),
            expected_view: genesis,
            kind: UpdateKind::Reconfigure { profile },
        },
        &budget(),
    )?)?;
    let source_ingress = client.begin_input(operation(607), &budget())?;
    let staged = client.stage_sources(
        source_ingress,
        vec![SourceInput {
            path: "shared.rs".into(),
            bytes: b"needle shared body".to_vec(),
            text: SourceTextAdmission::Excluded,
        }],
        &budget(),
    )?;
    let source = *staged.first().ok_or("same-kind source was not staged")?;
    let prior = client.read_view(ViewSelection::Exact(configured), &budget())?;
    let owners = client.stage_mutations(
        source_ingress,
        [OwnerMutation {
            owner: owner(1),
            expected: None,
            next: Some(source),
        }],
        &budget(),
    )?;
    let mut transition = SourceTransition {
        expected_source: prior.source,
        owners,
        events: Vec::new(),
    };
    transition.events.push(SourceEvent {
        producer: operation(607).producer,
        stream: operation(607).stream,
        stream_epoch: 1,
        predecessor: 0,
        sequence: 1,
        payload: transition.owner_digest()?,
    });
    let advanced = published(&client.submit(
        UpdateRequest {
            operation: operation(607),
            expected_view: configured,
            kind: UpdateKind::SourceAdvance(transition),
        },
        &budget(),
    )?)?;
    let current = client.read_view(ViewSelection::Exact(advanced), &budget())?;
    assert_ne!(current.members, prior.members);
    for spec in [&first, &second] {
        let page = client.search_literal(
            ViewSelection::Exact(advanced),
            spec.instance,
            b"needle",
            None,
            2,
            &budget(),
        )?;
        assert_eq!(page.view, advanced);
        assert_eq!(page.hits.len(), 1);
        let hit = page.hits.first().ok_or("same-kind literal hit absent")?;
        assert_eq!(hit.owner, owner(1));
        assert_eq!(hit.record.path, "shared.rs");
        assert!(!page.has_more);
        let old = client.search_literal(
            ViewSelection::Exact(configured),
            spec.instance,
            b"needle",
            None,
            2,
            &budget(),
        )?;
        assert_eq!(old.view, configured);
        assert!(old.hits.is_empty());
    }
    running.stop()?;

    // Inspect the persisted member rows independently of the query route.
    let catalog = Arc::new(SqliteCatalog::open(root.path(), Duration::from_secs(2))?);
    let scratch_root = private_root()?;
    let scratch = Arc::new(FileScratchSpace::open_standalone(
        scratch_root.path().join("scratch"),
        32 * 1024 * 1024,
    )?);
    let objects = FileObjectStore::open_managed_with_scratch(
        root.path().join("native-objects"),
        scratch,
        catalog,
    )?;
    let mut work = WorkBudget::with_request(work_limits(), budget());
    let mut member_references = Vec::new();
    for spec in [&first, &second] {
        let mut key = b"member\0".to_vec();
        key.extend_from_slice(&spec.instance.0);
        let reference = quanta_index_core::index::ports::PersistentMapPort::lookup(
            &objects,
            current.members,
            &key,
            &mut work,
        )?
        .ok_or("same-kind member row absent")?;
        let stored = objects.get(reference.object_ref()?, &mut work)?;
        let member: MemberBinding = serde_json::from_slice(&stored.payload)?;
        assert_eq!(stored.registration.reference, reference.object_ref()?);
        assert_eq!(member.instance, spec.instance);
        assert_eq!(member.format, spec.format);
        assert_eq!(member.input.schema, spec.schema);
        assert_eq!(member.input.policy, spec.policy);
        assert_eq!(member.input.root, current.source.owners);
        member_references.push(reference);
    }
    assert_ne!(member_references.first(), member_references.get(1));
    Ok(())
}

struct RefusingLiteralReader {
    need: ResourceNeed,
    calls: Arc<AtomicUsize>,
}

impl IndexQueryPort<LiteralReadRequest, LiteralPage> for RefusingLiteralReader {
    fn validate_instance(&self, _instance: &InstanceSpec) -> Result<(), IndexError> {
        Ok(())
    }

    fn reservation(&self, request: &LiteralReadRequest) -> Result<ResourceNeed, IndexError> {
        request.validate()?;
        Ok(self.need)
    }

    fn query(
        &self,
        _request: &LiteralReadRequest,
        _context: &mut QueryReadContext<'_>,
    ) -> Result<QueryOutcome<LiteralPage>, IndexError> {
        let _prior_calls = self.calls.fetch_add(1, Ordering::SeqCst);
        Err(IndexError::BackendViolation(
            "deliberate reader refusal".into(),
        ))
    }
}

#[test]
fn native_uds_literal_reader_failure_releases_pin_and_reservation() -> TestResult {
    let root = private_root()?;
    let socket = root.path().join("native.sock");
    let resources = test_resources();
    let need = ResourceNeed {
        provider_units: 0,
        ..resources.execution.interactive
    };
    let calls = Arc::new(AtomicUsize::new(0));
    let mut literal = QueryAdapterRegistry::new();
    literal.register(
        descriptors().literal,
        need,
        RefusingLiteralReader {
            need,
            calls: Arc::clone(&calls),
        },
    )?;
    let service = NativeIndexService::bind_with_capabilities(
        root.path(),
        &socket,
        work_limits(),
        NativeServiceCapabilities {
            builders: tokio_literal_registry()?,
            literal,
            path: QueryAdapterRegistry::new(),
            text: QueryAdapterRegistry::new(),
            semantic: QueryAdapterRegistry::new(),
            pump: None,
            provider_attempt_pool: None,
        },
        resources,
    )?;
    let observation = service.resource_observation();
    let running = NativeServiceGuard::start(service);
    let client = NativeUdsClient::new(socket);
    let created = client.request(
        NativeIndexRequest::CreateWorkspace {
            workspace: workspace(),
            epoch: 1,
        },
        &budget(),
    )?;
    let NativeIndexResponse::Created(genesis) = created else {
        return Err(format!("unexpected genesis reply: {created:?}").into());
    };
    let ingress = client.begin_input(operation(604), &budget())?;
    let profile = client.stage_profile(
        ingress,
        IndexProfile {
            instances: vec![literal_spec()],
        },
        &budget(),
    )?;
    let configured = published(&client.submit(
        UpdateRequest {
            operation: operation(604),
            expected_view: genesis,
            kind: UpdateKind::Reconfigure { profile },
        },
        &budget(),
    )?)?;
    assert!(matches!(
        client.search_literal(
            ViewSelection::Exact(configured),
            literal_spec().instance,
            b"needle",
            None,
            1,
            &budget(),
        ),
        Err(NativeUdsError::Refused(error))
            if error.code == NativeIndexErrorCode::BackendViolation
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(observation.observe()?.reserved, ResourceNeed::default());
    let catalog = SqliteCatalog::open(root.path(), Duration::from_secs(2))?;
    let pins: i64 = rusqlite::Connection::open(catalog.path())?.query_row(
        "SELECT COUNT(*) FROM native_pin_v1",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(pins, 0);
    running.stop()
}

#[test]
fn native_floor_only_memory_closes_replies_and_releases_pin_and_charges() -> TestResult {
    for preseeded in [false, true] {
        let root = private_root()?;
        let socket = root.path().join("native.sock");
        // Floor-only capacity has no escaping-data headroom. Exercise both
        // early creation refusal and an independently committed existing view.
        let seeded = if preseeded {
            let running = NativeServiceGuard::start(bind_service(
                root.path(),
                &socket,
                work_limits(),
                descriptors(),
            )?);
            let client = NativeUdsClient::new(socket.clone());
            let NativeIndexResponse::Created(view) = client.request(
                NativeIndexRequest::CreateWorkspace {
                    workspace: workspace(),
                    epoch: 1,
                },
                &budget(),
            )?
            else {
                return Err("seeded floor-only fixture did not create a view".into());
            };
            running.stop()?;
            Some(view)
        } else {
            None
        };
        let mut resources = test_resources();
        resources.execution.capacity.memory_bytes = resources
            .execution
            .control
            .memory_bytes
            .checked_add(resources.execution.interactive.memory_bytes)
            .and_then(|bytes| bytes.checked_add(resources.execution.core.memory_bytes))
            .and_then(|bytes| bytes.checked_add(resources.execution.enrichment.memory_bytes))
            .ok_or("floor-only fixture memory overflow")?;
        let service = NativeIndexService::bind(
            root.path(),
            &socket,
            work_limits(),
            descriptors(),
            resources,
        )?;
        let observation = service.resource_observation();
        let running = NativeServiceGuard::start(service);
        let client = NativeUdsClient::new(socket);
        let response = client.request(
            NativeIndexRequest::CreateWorkspace {
                workspace: workspace(),
                epoch: 1,
            },
            &budget(),
        );
        assert!(
            matches!(response, Err(NativeUdsError::Transport(message)) if message.contains("ipc frame truncated"))
        );
        if let Some(genesis) = seeded {
            assert!(
                matches!(client.search_literal(ViewSelection::Exact(genesis), literal_spec().instance, b"needle", None, 1, &budget()), Err(NativeUdsError::Transport(message)) if message.contains("ipc frame truncated"))
            );
            assert!(
                matches!(client.read_view(ViewSelection::Exact(genesis), &budget()), Err(NativeUdsError::Transport(message)) if message.contains("ipc frame truncated"))
            );
        }
        let after = observation.observe()?;
        assert_eq!(after.reserved, ResourceNeed::default());
        assert_eq!(after.live_memory_bytes, 0);
        assert_eq!(after.retained_memory_bytes, 0);
        assert_eq!(after.pending_escape_bytes, 0);
        let pins: i64 = rusqlite::Connection::open_with_flags(
            quanta_index_catalog::catalog_dir(root.path())
                .join(quanta_index_catalog::CATALOG_FILE_NAME),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )?
        .query_row("SELECT COUNT(*) FROM native_pin_v1", [], |row| row.get(0))?;
        assert_eq!(pins, 0);
        running.stop()?;
        // Catalog ticket acquisition writes a pin. Inspect the authenticated
        // latest view only after the managed service releases its namespace.
        let catalog = SqliteCatalog::open(root.path(), Duration::from_secs(2))?;
        let session = catalog.open_read_session()?;
        let selected = catalog.acquire_ticket(
            session.as_ref(),
            ViewSelection::Latest {
                workspace: workspace(),
                epoch: 1,
            },
            Duration::from_secs(5),
            &WorkBudget::with_request(work_limits(), budget()),
        );
        if let Some(genesis) = seeded {
            let ticket = selected?;
            assert_eq!(ticket.view.identity, genesis);
            catalog.release_ticket(session.as_ref(), ticket.lease)?;
        } else {
            assert!(matches!(selected, Err(IndexError::SnapshotUnavailable)));
        }
    }
    Ok(())
}

#[test]
fn native_literal_path_reader_ceiling_refuses_before_state_creation() -> TestResult {
    for path_reader in [false, true] {
        let root = private_root()?;
        let socket = root.path().join("native.sock");
        let resources = test_resources();
        let too_large = ResourceNeed {
            memory_bytes: resources
                .execution
                .interactive
                .memory_bytes
                .checked_add(1)
                .ok_or("interactive memory ceiling overflow")?,
            provider_units: 0,
            ..resources.execution.interactive
        };
        let mut literal = QueryAdapterRegistry::new();
        let mut path = QueryAdapterRegistry::new();
        if path_reader {
            path.register(
                descriptors().file_map,
                too_large,
                NativePathReader::new(too_large)?,
            )?;
        } else {
            literal.register(
                descriptors().literal,
                too_large,
                NativeLiteralReader::new(too_large)?,
            )?;
        }
        assert!(
            NativeIndexService::bind_with_capabilities(
                root.path(),
                &socket,
                work_limits(),
                NativeServiceCapabilities {
                    builders: AdapterRegistry::new(),
                    literal,
                    path,
                    text: QueryAdapterRegistry::new(),
                    semantic: QueryAdapterRegistry::new(),
                    pump: None,
                    provider_attempt_pool: None,
                },
                resources,
            )
            .is_err()
        );
        assert!(
            !root
                .path()
                .join(quanta_index_searchd_runtime::native::NATIVE_MARKER_NAME)
                .exists()
        );
        assert!(!root.path().join("catalog").exists());
    }
    Ok(())
}

fn budget() -> RequestBudgetV1 {
    RequestBudgetV1::for_duration(Duration::from_secs(30))
}

fn published(outcome: &SubmitOutcome) -> TestResult<quanta_index_contract::index::ViewRef> {
    let SubmitOutcome::Terminal(OperationStatus::Published(view)) = outcome else {
        return Err(format!("expected published native view, got {outcome:?}").into());
    };
    Ok(*view)
}

struct NativeServiceGuard {
    shutdown: quanta_index_ipc::ShutdownHandle,
    thread: Option<JoinHandle<anyhow::Result<()>>>,
    observation: NativeWorkerObservation,
    resources: NativeResourceObservation,
}

impl NativeServiceGuard {
    fn start(service: NativeIndexService) -> Self {
        let shutdown = service.shutdown_handle();
        let observation = service.worker_observation();
        let resources = service.resource_observation();
        let thread = std::thread::spawn(move || service.run(Duration::from_millis(5)));
        Self {
            shutdown,
            thread: Some(thread),
            observation,
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
        assert_eq!(self.resources.observe()?.reserved, ResourceNeed::default());
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
        drop(released);
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
            .map_err(|_panic| "materialize client thread panicked")??)
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

    fn resource_demand(&self) -> ResourceNeed {
        ResourceNeed::default()
    }

    fn validate_instance(
        &self,
        _instance: &quanta_index_core::index::InstanceSpec,
    ) -> Result<(), IndexError> {
        Ok(())
    }

    fn prepare_owner<'a>(
        &'a self,
        instance: &'a InstanceSpec,
        owner: OwnerKey,
        input: OwnerInputBinding,
        _prior: Option<OwnerMemberBinding>,
        context: &'a mut AttemptContext<'_>,
    ) -> IndexFuture<'a, Self::Candidate> {
        Box::pin(async move {
            context.budget.checkpoint()?;
            self.gate.wait()?;
            context.budget.checkpoint()?;
            let source_ref = input.source.ok_or_else(|| {
                IndexError::InvalidInput("paused owner requires a source record".into())
            })?;
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
                || u64::try_from(bytes.payload.len())
                    .map_err(|_conversion_error| IndexError::ResourceLimited)?
                    != record.byte_len
            {
                return Err(IndexError::CorruptObject("owner bytes differ".into()));
            }
            let payload = serde_json::to_vec(&(owner, input, record.path, &bytes.payload))
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
    let reference = client.stage_source(
        ingress,
        path.into(),
        bytes,
        SourceTextAdmission::Excluded,
        &budget(),
    )?;
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
        predecessor: event_sequence
            .checked_sub(1)
            .ok_or("source event predecessor underflow")?,
        sequence: event_sequence,
        payload: transition.owner_digest()?,
    });
    published(&client.submit(
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
    let service = bind_service_with_registry(root.path(), &socket, work_limits(), registry)?;
    let service = NativeServiceGuard::start(service);
    // This adapter blocks locally and declares no external provider demand.
    // The configured enrichment class protects one provider unit, but the
    // reservation for this execution uses its declared zero-provider demand.
    let owner_reservation = ResourceNeed {
        provider_units: 0,
        ..test_resources().execution.enrichment
    };
    // This guard releases the adapter before service teardown on every error path.
    let _release_on_drop = ReleaseOnDrop(Arc::clone(&gate));
    let client = NativeUdsClient::new(socket.clone());
    let genesis_reply = client.request(
        NativeIndexRequest::CreateWorkspace {
            workspace: workspace(),
            epoch: 1,
        },
        &budget(),
    )?;
    let NativeIndexResponse::Created(genesis) = genesis_reply else {
        return Err(format!("expected genesis, got {genesis_reply:?}").into());
    };
    let profile_ingress = client.begin_input(operation(1), &budget())?;
    let profile = client.stage_profile(
        profile_ingress,
        IndexProfile {
            instances: vec![literal_spec(), owner_spec],
        },
        &budget(),
    )?;
    let configured = published(&client.submit(
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
    let ready_reply = client.request(
        NativeIndexRequest::ReadyWork {
            workspace: workspace(),
            epoch: 1,
            limit: 16,
        },
        &budget(),
    )?;
    let NativeIndexResponse::ReadyWork(ready) = ready_reply else {
        return Err(format!("expected ready work, got {ready_reply:?}").into());
    };
    let intent: WorkIntent = ready
        .into_iter()
        .find(|candidate| {
            candidate.instance == owner_instance
                && matches!(candidate.scope, WorkScope::Owner { owner: work_owner, .. } if work_owner == owner(1))
        })
        .ok_or("owner A work was not enqueued")?;
    let claim_reply = client.request(
        NativeIndexRequest::ClaimWork {
            intent,
            lease_millis: 60_000,
        },
        &budget(),
    )?;
    let NativeIndexResponse::Claimed(claim) = claim_reply else {
        return Err(format!("expected work claim, got {claim_reply:?}").into());
    };
    let materialize_key = OperationKey {
        producer: ProducerId([41; 16]),
        stream: StreamId([42; 16]),
        sequence: 1,
        ..operation(1)
    };
    let materialize_socket = socket;
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
        .map_err(|error| format!("materialize did not enter owner adapter: {error}"))?;
    let observed = service.observation.observe(Duration::from_secs(1))?;
    assert_eq!(observed.status, NativeWorkerStatus::Running);
    assert_eq!(
        (observed.enrichment.queued, observed.enrichment.active),
        (0, 1)
    );
    assert_eq!(service.resources.observe()?.reserved, owner_reservation);
    assert!(matches!(
        client.status(materialize_key, &budget())?,
        OperationStatus::Pending(_)
    ));
    let renew_reply = client.request(
        NativeIndexRequest::RenewWork {
            claim,
            lease_millis: 60_000,
        },
        &budget(),
    )?;
    let NativeIndexResponse::Renewed(renewed) = renew_reply else {
        return Err(format!("expected renewed blocked-owner claim, got {renew_reply:?}").into());
    };
    assert_eq!(renewed.intent, claim.intent);
    assert_eq!(renewed.fence, claim.fence);
    assert!(renewed.lease_until_ms >= claim.lease_until_ms);
    assert_eq!(
        client.request(NativeIndexRequest::WorkStatus(intent), &budget())?,
        NativeIndexResponse::WorkStatus(WorkStatus::Claimed(renewed))
    );
    assert_eq!(service.resources.observe()?.reserved, owner_reservation);

    // The owner adapter is synchronously blocked on one Tokio worker. The
    // unrelated source update must complete through the same real UDS socket.
    let view_b = advance_one(&client, ingress_b, view_a, owner(2), source_b, 2)?;
    assert_eq!(service.resources.observe()?.reserved, owner_reservation);
    let historical_a = client.read_view(ViewSelection::Exact(view_a), &budget())?;
    let historical_b = client.read_view(ViewSelection::Exact(view_b), &budget())?;
    assert_ne!(historical_a.source, historical_b.source);
    assert_ne!(historical_a.members, historical_b.members);
    assert_eq!(
        client.status(operation(3), &budget())?,
        OperationStatus::Published(view_b)
    );
    let materialized = published(&materialize.finish()?)?;
    assert_eq!(
        service.resources.observe()?.reserved,
        ResourceNeed::default()
    );
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
    let service = bind_service_with_registry(
        root.path(),
        &socket,
        work_limits(),
        tokio_literal_registry()?,
    )?;
    assert!(bind_service(root.path(), &socket, work_limits(), descriptors()).is_err());
    let shutdown = service.shutdown_handle();
    let worker = std::thread::spawn(move || service.run(Duration::from_millis(5)));
    let client = NativeUdsClient::new(socket.clone());
    let genesis_reply = client.request(
        NativeIndexRequest::CreateWorkspace {
            workspace: workspace(),
            epoch: 1,
        },
        &budget(),
    )?;
    let NativeIndexResponse::Created(genesis) = genesis_reply else {
        return Err(format!("unexpected genesis reply: {genesis_reply:?}").into());
    };
    let profile_ingress = client.begin_input(operation(1), &budget())?;
    let profile = client.stage_profile(
        profile_ingress,
        IndexProfile {
            instances: vec![literal_spec(), file_spec()],
        },
        &budget(),
    )?;
    let configured = published(&client.submit(
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
                text: SourceTextAdmission::Utf8 {
                    policy: Digest([42; 32]),
                },
            },
            SourceInput {
                path: "b.rs".into(),
                bytes: b"second native needle".to_vec(),
                text: SourceTextAdmission::Excluded,
            },
            SourceInput {
                path: "a.rs".into(),
                bytes: b"hello native needle".to_vec(),
                text: SourceTextAdmission::Utf8 {
                    policy: Digest([42; 32]),
                },
            },
        ],
        &budget(),
    )?;
    assert_eq!(staged.len(), 3);
    let first = *staged.first().ok_or("first staged source absent")?;
    let second = *staged.get(1).ok_or("second staged source absent")?;
    let duplicate = *staged.get(2).ok_or("duplicate staged source absent")?;
    assert_eq!(first, duplicate);
    assert_ne!(first, second);
    let staged_reply = client.request(
        NativeIndexRequest::StageSource {
            ingress: source_ingress,
            path: "a.rs".into(),
            bytes: b"hello native needle".to_vec(),
            text: SourceTextAdmission::Utf8 {
                policy: Digest([42; 32]),
            },
        },
        &budget(),
    )?;
    let NativeIndexResponse::Staged(direct) = staged_reply else {
        return Err(format!("unexpected direct source stage: {staged_reply:?}").into());
    };
    assert_eq!(direct, first);
    let record = first;
    let second_record = second;
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
    let next = published(&client.submit(
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
    let first_literal_hit = literal_page.hits.first().ok_or("literal hit absent")?;
    assert_eq!(first_literal_hit.record.path, "a.rs");
    assert_eq!(
        first_literal_hit.record.text,
        SourceTextAdmission::Utf8 {
            policy: Digest([42; 32]),
        }
    );
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
    assert_eq!(
        literal_next
            .hits
            .first()
            .ok_or("next literal hit absent")?
            .record
            .path,
        "b.rs"
    );
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
    assert_eq!(
        path_page.entries.first().ok_or("path entry absent")?.path,
        "a.rs"
    );
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
        Some(
            &paths_first
                .entries
                .first()
                .ok_or("first path entry absent")?
                .path,
        ),
        1,
        &budget(),
    )?;
    assert_eq!(paths_second.entries.len(), 1);
    assert_eq!(
        paths_second
            .entries
            .first()
            .ok_or("second path entry absent")?
            .path,
        "b.rs"
    );
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
    let projected = published(&client.submit(
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
        .map_err(|_panic| "native service thread panicked")??;
    let reopened = bind_service(root.path(), &socket, work_limits(), descriptors())?;
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
    let reopened_hit = reopened_literal
        .hits
        .first()
        .ok_or("reopened literal hit absent")?;
    assert_eq!(reopened_hit.record.path, "a.rs");
    assert_eq!(
        reopened_hit.record.text,
        SourceTextAdmission::Utf8 {
            policy: Digest([42; 32]),
        }
    );
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
    assert_eq!(
        reopened_paths
            .entries
            .first()
            .ok_or("reopened path absent")?
            .path,
        "a.rs"
    );
    stop.trigger();
    worker
        .join()
        .map_err(|_panic| "reopened service thread panicked")??;
    Ok(())
}

fn native_text_spec() -> InstanceSpec {
    let recipe = TextRecipe::new(Digest([31; 32]));
    InstanceSpec {
        instance: IndexInstanceId([71; 16]),
        kind: IndexKind([72; 16]),
        group: PublicationGroupId([73; 16]),
        activation: GroupActivation::CoreRequired,
        input_domain: InputDomain::SourceLocal,
        schema: SchemaId([74; 16]),
        policy: recipe.output_policy(),
        format: StorageFormatId([75; 16]),
    }
}

fn bind_native_text_service(
    root: &std::path::Path,
    socket: &std::path::Path,
) -> anyhow::Result<NativeIndexService> {
    let resources = test_resources();
    bind_native_text_service_with_resources(
        root,
        socket,
        resources,
        resources.execution.interactive,
    )
}

fn bind_native_text_service_with_resources(
    root: &std::path::Path,
    socket: &std::path::Path,
    resources: NativeServiceResources,
    read_need: ResourceNeed,
) -> anyhow::Result<NativeIndexService> {
    let recipe = TextRecipe::new(Digest([31; 32]));
    let spec = native_text_spec();
    let descriptor = AdapterDescriptor {
        kind: spec.kind,
        schema: spec.schema,
        format: spec.format,
    };
    let mut builders = AdapterRegistry::new();
    builders.register(descriptor, NativeTextIndex::new(recipe), |delta| {
        Ok(delta.clone())
    })?;
    let mut readers = QueryAdapterRegistry::<PlannedTextRead, TextReadResult>::new();
    readers.register(
        descriptor,
        read_need,
        NativeTextReader::new(recipe, Bm25Policy { k1: 1.2, b: 0.75 }, read_need)?,
    )?;
    NativeIndexService::bind_with_registry_and_queries(
        root,
        socket,
        work_limits(),
        builders,
        readers,
        resources,
    )
}

fn native_text_request(text: &str, top_k: u32) -> TextQueryRequest {
    TextQueryRequest {
        syntax: TextQuerySyntax::Native,
        query_text: text.into(),
        constraints: QueryConstraintSetV1::unconstrained(),
        generation: None,
        generation_selector: None,
        top_k,
        cursor: None,
    }
}

#[test]
fn native_uds_text_query_uses_selected_reader_need_below_interactive_envelope() -> TestResult {
    let root = private_root()?;
    let socket = root.path().join("native.sock");
    let mut resources = test_resources();
    resources.execution.interactive.provider_units = 1;
    resources.execution.capacity.provider_units = 2;
    let read_need = ResourceNeed {
        provider_units: 0,
        ..resources.execution.interactive
    };
    let service = NativeServiceGuard::start(bind_native_text_service_with_resources(
        root.path(),
        &socket,
        resources,
        read_need,
    )?);
    let client = NativeUdsClient::new(socket);
    let NativeIndexResponse::Created(genesis) = client.request(
        NativeIndexRequest::CreateWorkspace {
            workspace: workspace(),
            epoch: 1,
        },
        &budget(),
    )?
    else {
        return Err("native text genesis reply differs".into());
    };
    let ingress = client.begin_input(operation(1), &budget())?;
    let profile = client.stage_profile(
        ingress,
        IndexProfile {
            instances: vec![native_text_spec()],
        },
        &budget(),
    )?;
    let configured = published(&client.submit(
        UpdateRequest {
            operation: operation(1),
            expected_view: genesis,
            kind: UpdateKind::Reconfigure { profile },
        },
        &budget(),
    )?)?;
    let result = client.search_text(
        ViewSelection::Exact(configured),
        native_text_spec().instance,
        native_text_request("one", 2),
        &budget(),
    )?;
    assert_eq!(result.view, configured);
    assert_eq!(result.instance, native_text_spec().instance);
    assert_eq!(result.coverage, NativeTextCoverage::CoreRequired);
    assert!(result.exhaustive);
    assert_eq!(result.matched, 0);
    assert!(result.hits.is_empty());
    service.stop()
}

/// Fixed BM25 gold crosses the real SDK/UDS, selected profile, source and
/// native text member; an old view and a reopened service must retain identity.
#[test]
fn native_uds_text_query_binds_pinned_view_and_fixed_bm25_gold() -> TestResult {
    let root = private_root()?;
    let socket = root.path().join("native.sock");
    let service = NativeServiceGuard::start(bind_native_text_service(root.path(), &socket)?);
    let client = NativeUdsClient::new(socket.clone());
    let NativeIndexResponse::Created(genesis) = client.request(
        NativeIndexRequest::CreateWorkspace {
            workspace: workspace(),
            epoch: 1,
        },
        &budget(),
    )?
    else {
        return Err("native text genesis reply differs".into());
    };
    let profile_ingress = client.begin_input(operation(1), &budget())?;
    let profile = client.stage_profile(
        profile_ingress,
        IndexProfile {
            instances: vec![native_text_spec()],
        },
        &budget(),
    )?;
    let configured = published(&client.submit(
        UpdateRequest {
            operation: operation(1),
            expected_view: genesis,
            kind: UpdateKind::Reconfigure { profile },
        },
        &budget(),
    )?)?;
    let source_ingress = client.begin_input(operation(2), &budget())?;
    let staged = client.stage_sources(
        source_ingress,
        vec![
            SourceInput {
                path: "a.rs".into(),
                bytes: "Café CAFÉ one one".as_bytes().to_vec(),
                text: SourceTextAdmission::Utf8 {
                    policy: Digest([31; 32]),
                },
            },
            SourceInput {
                path: "b.rs".into(),
                bytes: "cafe\u{301} one".as_bytes().to_vec(),
                text: SourceTextAdmission::Utf8 {
                    policy: Digest([31; 32]),
                },
            },
            SourceInput {
                path: "empty.rs".into(),
                bytes: Vec::new(),
                text: SourceTextAdmission::Utf8 {
                    policy: Digest([31; 32]),
                },
            },
            SourceInput {
                path: "binary.rs".into(),
                bytes: vec![0xff],
                text: SourceTextAdmission::Excluded,
            },
        ],
        &budget(),
    )?;
    assert_eq!(staged.len(), 4);
    let [source_a, source_b, _excluded_path, _excluded_binary] = staged.as_slice() else {
        return Err("native text fixture must stage four sources".into());
    };
    let prior = client.read_view(ViewSelection::Exact(configured), &budget())?;
    let owners = client.stage_mutations(
        source_ingress,
        [1_u8, 2, 3, 4]
            .into_iter()
            .zip(&staged)
            .map(|(id, reference)| OwnerMutation {
                owner: OwnerKey {
                    producer: operation(2).producer,
                    stable_id: StableOwnerId([id; 16]),
                },
                expected: None,
                next: Some(*reference),
            }),
        &budget(),
    )?;
    let mut transition = SourceTransition {
        expected_source: prior.source,
        owners,
        events: Vec::new(),
    };
    transition.events.push(SourceEvent {
        producer: operation(2).producer,
        stream: operation(2).stream,
        stream_epoch: 1,
        predecessor: 0,
        sequence: 1,
        payload: transition.owner_digest()?,
    });
    let published_view = published(&client.submit(
        UpdateRequest {
            operation: operation(2),
            expected_view: configured,
            kind: UpdateKind::SourceAdvance(transition),
        },
        &budget(),
    )?)?;
    let ranked = client.search_text(
        ViewSelection::Exact(published_view),
        native_text_spec().instance,
        native_text_request("one", 2),
        &budget(),
    )?;
    assert_eq!(ranked.view, published_view);
    assert_eq!(ranked.instance, native_text_spec().instance);
    assert_eq!(ranked.coverage, NativeTextCoverage::CoreRequired);
    assert!(ranked.exhaustive);
    assert_eq!(ranked.matched, 2);
    assert_eq!(ranked.hits.len(), 2);
    for (hit, (expected_owner, expected_source, expected_path, expected_score)) in
        ranked.hits.iter().zip([
            (1_u8, *source_a, "a.rs", 0.504_394_138_702_740_6_f64),
            (2_u8, *source_b, "b.rs", 0.470_003_629_245_735_5_f64),
        ])
    {
        assert_eq!(hit.owner.stable_id, StableOwnerId([expected_owner; 16]));
        assert_eq!(
            (hit.source, hit.path.as_str()),
            (expected_source, expected_path)
        );
        assert!(hit.score.get().to_bits().abs_diff(expected_score.to_bits()) <= 16);
    }
    let counted = client.search_text(
        ViewSelection::Exact(published_view),
        native_text_spec().instance,
        native_text_request("one count:1", 2),
        &budget(),
    )?;
    if counted.matched != 2
        || counted.hits.len() != 1
        || counted.hits.first() != ranked.hits.first()
        || !counted.exhaustive
    {
        return Err(
            "Native count:1 changed matched total or failed the SDK cardinality join".into(),
        );
    }
    let mut path_query = native_text_request("one", 1);
    let negative = client.search_text(
        ViewSelection::Exact(published_view),
        native_text_spec().instance,
        native_text_request("NOT one count:1", 2),
        &budget(),
    )?;
    let [empty_hit] = negative.hits.as_slice() else {
        return Err("NOT one must return exactly the admitted empty document".into());
    };
    if negative.matched != 1
        || !negative.exhaustive
        || empty_hit.owner.stable_id != StableOwnerId([3; 16])
        || empty_hit.path != "empty.rs"
        || empty_hit.score.get() != 0.0
    {
        return Err(
            "unanchored UDS query included excluded source or lost zero-score empty text".into(),
        );
    }
    let union = client.search_text(
        ViewSelection::Exact(published_view),
        native_text_spec().instance,
        native_text_request("one OR NOT absent", 3),
        &budget(),
    )?;
    if union.matched != 3
        || !union.exhaustive
        || union.hits.len() != 3
        || union.hits.get(..2) != Some(ranked.hits.as_slice())
        || union.hits.last() != Some(empty_hit)
    {
        return Err("OR NOT changed fixed BM25 ranking or skipped the admitted universe".into());
    }
    path_query.constraints =
        quanta_index_contract::QueryConstraintSetV1::from_exact_repo_relative_path(
            quanta_index_contract::ExactRepoRelativePathV1::new("b.rs")?,
        );
    let filtered = client.search_text(
        ViewSelection::Exact(published_view),
        native_text_spec().instance,
        path_query.clone(),
        &budget(),
    )?;
    assert_eq!(filtered.matched, 1);
    assert!(filtered.exhaustive);
    assert_eq!(
        filtered.hits.as_slice(),
        ranked
            .hits
            .get(1..2)
            .ok_or("missing fixed lower-ranked path hit")?
    );
    let old_filtered = client.search_text(
        ViewSelection::Exact(configured),
        native_text_spec().instance,
        path_query.clone(),
        &budget(),
    )?;
    assert_eq!(old_filtered.matched, 0);
    assert!(old_filtered.hits.is_empty());
    let nested = client.search_text(
        ViewSelection::Exact(published_view),
        native_text_spec().instance,
        native_text_request("(one OR absent) AND one", 2),
        &budget(),
    )?;
    assert_eq!(nested.matched, 2);
    assert_eq!(nested.hits, ranked.hits);
    // Fixed source gold: only a.rs has adjacent repeated "one" tokens.
    // Phrase matching preserves the existing unique-term BM25 score.
    let phrase = client.search_text(
        ViewSelection::Exact(published_view),
        native_text_spec().instance,
        native_text_request("\"one one\"", 2),
        &budget(),
    )?;
    assert!(phrase.exhaustive);
    assert_eq!(phrase.matched, 1);
    assert_eq!(
        phrase.hits,
        ranked.hits.get(..1).ok_or("missing fixed phrase hit")?
    );
    let reversed = client.search_text(
        ViewSelection::Exact(published_view),
        native_text_spec().instance,
        native_text_request("\"one café\"", 2),
        &budget(),
    )?;
    assert_eq!(reversed.matched, 0);
    assert!(reversed.hits.is_empty());
    let old_phrase = client.search_text(
        ViewSelection::Exact(configured),
        native_text_spec().instance,
        native_text_request("\"one one\"", 2),
        &budget(),
    )?;
    assert_eq!(old_phrase.matched, 0);
    assert!(old_phrase.hits.is_empty());
    let old = client.search_text(
        ViewSelection::Exact(configured),
        native_text_spec().instance,
        native_text_request("one", 2),
        &budget(),
    )?;
    assert_eq!(old.view, configured);
    assert_eq!(old.matched, 0);
    assert!(old.hits.is_empty());
    assert!(matches!(
        client.request(
            NativeIndexRequest::SearchText {
                selection: ViewSelection::Exact(published_view),
                instance: native_text_spec().instance,
                query: native_text_request("one", 1025),
            },
            &budget(),
        ),
        Err(NativeUdsError::Refused(error)) if error.code == NativeIndexErrorCode::InvalidInput
    ));
    let mut unsupported = native_text_request("one", 2);
    unsupported.syntax = TextQuerySyntax::CodeSearch;
    assert!(matches!(
        client.request(
            NativeIndexRequest::SearchText {
                selection: ViewSelection::Exact(published_view),
                instance: native_text_spec().instance,
                query: unsupported,
            },
            &budget(),
        ),
        Err(NativeUdsError::Refused(error)) if error.code == NativeIndexErrorCode::InvalidInput
    ));
    service.stop()?;
    let reopened = NativeServiceGuard::start(bind_native_text_service(root.path(), &socket)?);
    let replay = client.search_text(
        ViewSelection::Exact(published_view),
        native_text_spec().instance,
        native_text_request("one", 2),
        &budget(),
    )?;
    assert_eq!(replay, ranked);
    let replay_phrase = client.search_text(
        ViewSelection::Exact(published_view),
        native_text_spec().instance,
        native_text_request("\"one one\"", 2),
        &budget(),
    )?;
    assert_eq!(replay_phrase, phrase);
    let filtered_replay = client.search_text(
        ViewSelection::Exact(published_view),
        native_text_spec().instance,
        path_query,
        &budget(),
    )?;
    assert_eq!(filtered_replay, filtered);
    reopened.stop()
}

#[test]
fn native_uds_resumes_original_pending_request_after_reopen() -> TestResult {
    let root = private_root()?;
    let socket = root.path().join("native.sock");
    let service = bind_service(root.path(), &socket, work_limits(), descriptors())?;
    let shutdown = service.shutdown_handle();
    let worker = std::thread::spawn(move || service.run(Duration::from_millis(5)));
    let client = NativeUdsClient::new(socket.clone());
    let genesis_reply = client.request(
        NativeIndexRequest::CreateWorkspace {
            workspace: workspace(),
            epoch: 1,
        },
        &budget(),
    )?;
    let NativeIndexResponse::Created(genesis) = genesis_reply else {
        return Err(format!("unexpected genesis reply: {genesis_reply:?}").into());
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
        .map_err(|_panic| "native service thread panicked")??;

    // The public durable journal admits an original immutable request before
    // execution. No product test hook or fabricated Pending status is needed.
    let catalog = Arc::new(SqliteCatalog::open(root.path(), Duration::from_secs(2))?);
    let scratch_root = private_root()?;
    let scratch = Arc::new(FileScratchSpace::open_standalone(
        scratch_root.path().join("scratch"),
        32 * 1024 * 1024,
    )?);
    let objects = FileObjectStore::open_managed_with_scratch(
        root.path().join("native-objects"),
        scratch,
        catalog.clone(),
    )?;
    let mut work = WorkBudget::with_request(work_limits(), budget());
    let request = UpdateRequest {
        operation: operation(1),
        expected_view: genesis,
        kind: UpdateKind::Reconfigure { profile },
    };
    let request_bytes = serde_json::to_vec(&request)?;
    let sink = FixtureIngressSink {
        catalog: catalog.as_ref(),
        ingress: profile_ingress,
    };
    let request_object = objects.install(&request_bytes, &[profile], &sink, &mut work)?;
    catalog.seal_inputs(profile_ingress, request_object.reference)?;
    let admitted = catalog.admit(
        ImmutableOperation {
            key: request.operation,
            request: request_object.reference,
            source_mode: request.kind.source_mode(),
        },
        profile_ingress,
        &[],
        &mut work,
    )?;
    assert!(matches!(admitted, AdmissionOutcome::New(_)));
    assert!(matches!(
        catalog.inspect(
            request.operation,
            quanta_index_core::index::ports::OperationInspectAccess::Request(
                &WorkBudget::with_request(work_limits(), budget()),
            ),
        )?,
        OperationStatus::Pending(_)
    ));
    drop(catalog);
    drop(objects);

    let reopened = bind_service(root.path(), &socket, work_limits(), descriptors())?;
    let stop = reopened.shutdown_handle();
    let worker = std::thread::spawn(move || reopened.run(Duration::from_millis(5)));
    assert!(matches!(
        client.status(request.operation, &budget())?,
        OperationStatus::Pending(_)
    ));
    let published = published(&client.resume(request.operation, &budget())?)?;
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
        .map_err(|_panic| "reopened service thread panicked")??;
    Ok(())
}

#[test]
fn native_format_refuses_v6_before_initializing_catalog() -> TestResult {
    let prior = private_root()?;
    let marker = prior.path().join(".native-index-v6");
    std::fs::write(&marker, b"quanta-index-native-state-v6\n")?;
    std::fs::set_permissions(&marker, std::fs::Permissions::from_mode(0o600))?;
    let error = match bind_service(
        prior.path(),
        &prior.path().join("native.sock"),
        work_limits(),
        descriptors(),
    ) {
        Ok(_service) => return Err("v6 state without installation custody was accepted".into()),
        Err(error) => error,
    };
    if !error.to_string().contains("explicit rebuild") {
        return Err(format!("v6 refusal omitted explicit rebuild: {error}").into());
    }
    if std::fs::read(marker)? != b"quanta-index-native-state-v6\n" {
        return Err("v6 refusal changed the original marker".into());
    }
    for entry in [".native-index-v18", "catalog", "native-objects"] {
        if prior.path().join(entry).try_exists()? {
            return Err(format!("v6 refusal initialized {entry}").into());
        }
    }
    Ok(())
}

#[test]
fn native_format_refuses_v7_before_initializing_bootstrap_catalog() -> TestResult {
    let prior = private_root()?;
    let marker = prior.path().join(".native-index-v7");
    std::fs::write(&marker, b"quanta-index-native-state-v7\n")?;
    std::fs::set_permissions(&marker, std::fs::Permissions::from_mode(0o600))?;
    let error = match bind_service(
        prior.path(),
        &prior.path().join("native.sock"),
        work_limits(),
        descriptors(),
    ) {
        Ok(_service) => return Err("v7 state without bootstrap custody was accepted".into()),
        Err(error) => error,
    };
    if !error.to_string().contains("explicit rebuild") {
        return Err(format!("v7 refusal omitted explicit rebuild: {error}").into());
    }
    if std::fs::read(marker)? != b"quanta-index-native-state-v7\n" {
        return Err("v7 refusal changed the original marker".into());
    }
    for entry in [".native-index-v18", "catalog", "native-objects"] {
        if prior.path().join(entry).try_exists()? {
            return Err(format!("v7 refusal initialized {entry}").into());
        }
    }
    Ok(())
}

#[test]
fn native_format_refuses_v8_before_initializing_gc_catalog_or_objects() -> TestResult {
    let prior = private_root()?;
    let marker = prior.path().join(".native-index-v8");
    std::fs::write(&marker, b"quanta-index-native-state-v8\n")?;
    std::fs::set_permissions(&marker, std::fs::Permissions::from_mode(0o600))?;
    let error = match bind_service(
        prior.path(),
        &prior.path().join("native.sock"),
        work_limits(),
        descriptors(),
    ) {
        Ok(_service) => return Err("v8 state without physical GC lifecycle was accepted".into()),
        Err(error) => error,
    };
    if !error.to_string().contains("explicit rebuild") {
        return Err(format!("v8 refusal omitted explicit rebuild: {error}").into());
    }
    if std::fs::read(&marker)? != b"quanta-index-native-state-v8\n" {
        return Err("v8 refusal changed the original marker".into());
    }
    for entry in [
        ".native-index-v18",
        "catalog",
        "native-objects",
        "native-scratch-v1",
    ] {
        if prior.path().join(entry).try_exists()? {
            return Err(format!("v8 refusal initialized {entry}").into());
        }
    }
    Ok(())
}

#[test]
fn native_format_refuses_unmarked_existing_data_and_symlink_marker() -> TestResult {
    let native = private_root()?;
    let native_service = bind_service(
        native.path(),
        &native.path().join("native.sock"),
        work_limits(),
        descriptors(),
    )?;
    assert_eq!(
        std::fs::read(native.path().join(".native-index-v18"))?,
        b"quanta-index-native-state-v18\n"
    );
    assert!(matches!(
        quanta_index_searchd::app::state_format::refuse_legacy_state_root_v1(native.path()),
        Err(quanta_index_core::CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::StateRootFormatUnsupported,
            ..
        })
    ));
    drop(native_service);

    let old = private_root()?;
    std::fs::write(old.path().join("old-data"), b"legacy")?;
    assert!(
        bind_service(
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
    let prior_error = match bind_service(
        prior.path(),
        &prior.path().join("native.sock"),
        work_limits(),
        descriptors(),
    ) {
        Ok(_service) => return Err("v1 native state unexpectedly reopened".into()),
        Err(error) => error,
    };
    assert!(prior_error.to_string().contains("explicit rebuild"));
    assert!(!prior.path().join(".native-index-v18").exists());

    let prior_v2 = private_root()?;
    let prior_v2_marker = prior_v2.path().join(".native-index-v2");
    std::fs::write(&prior_v2_marker, b"quanta-index-native-state-v2\n")?;
    std::fs::set_permissions(&prior_v2_marker, std::fs::Permissions::from_mode(0o600))?;
    let prior_v2_error = match bind_service(
        prior_v2.path(),
        &prior_v2.path().join("native.sock"),
        work_limits(),
        descriptors(),
    ) {
        Ok(_service) => return Err("v2 native state unexpectedly reopened".into()),
        Err(error) => error,
    };
    assert!(prior_v2_error.to_string().contains("explicit rebuild"));
    assert!(!prior_v2.path().join(".native-index-v18").exists());

    let prior_v3 = private_root()?;
    let prior_v3_marker = prior_v3.path().join(".native-index-v3");
    std::fs::write(&prior_v3_marker, b"quanta-index-native-state-v3\n")?;
    std::fs::set_permissions(&prior_v3_marker, std::fs::Permissions::from_mode(0o600))?;
    let prior_v3_error = match bind_service(
        prior_v3.path(),
        &prior_v3.path().join("native.sock"),
        work_limits(),
        descriptors(),
    ) {
        Ok(_service) => return Err("v3 native state unexpectedly reopened".into()),
        Err(error) => error,
    };
    assert!(prior_v3_error.to_string().contains("explicit rebuild"));
    assert!(!prior_v3.path().join(".native-index-v18").exists());

    let prior_v4 = private_root()?;
    let prior_v4_marker = prior_v4.path().join(".native-index-v4");
    std::fs::write(&prior_v4_marker, b"quanta-index-native-state-v4\n")?;
    std::fs::set_permissions(&prior_v4_marker, std::fs::Permissions::from_mode(0o600))?;
    let prior_v4_error = match bind_service(
        prior_v4.path(),
        &prior_v4.path().join("native.sock"),
        work_limits(),
        descriptors(),
    ) {
        Ok(_service) => return Err("v4 native state unexpectedly reopened".into()),
        Err(error) => error,
    };
    assert!(prior_v4_error.to_string().contains("explicit rebuild"));
    assert_eq!(
        std::fs::read(&prior_v4_marker)?,
        b"quanta-index-native-state-v4\n"
    );
    assert!(!prior_v4.path().join(".native-index-v18").exists());
    assert!(!prior_v4.path().join("catalog").exists());

    let prior_v5 = private_root()?;
    let prior_v5_marker = prior_v5.path().join(".native-index-v5");
    std::fs::write(&prior_v5_marker, b"quanta-index-native-state-v5\n")?;
    std::fs::set_permissions(&prior_v5_marker, std::fs::Permissions::from_mode(0o600))?;
    let prior_v5_error = match bind_service(
        prior_v5.path(),
        &prior_v5.path().join("native.sock"),
        work_limits(),
        descriptors(),
    ) {
        Ok(_service) => return Err("v5 native state unexpectedly reopened".into()),
        Err(error) => error,
    };
    assert!(prior_v5_error.to_string().contains("explicit rebuild"));
    assert_eq!(
        std::fs::read(&prior_v5_marker)?,
        b"quanta-index-native-state-v5\n"
    );
    assert!(!prior_v5.path().join(".native-index-v18").exists());
    assert!(!prior_v5.path().join("catalog").exists());

    let symlinked = private_root()?;
    std::fs::write(
        symlinked.path().join("target"),
        b"quanta-index-native-state-v18\n",
    )?;
    std::os::unix::fs::symlink("target", symlinked.path().join(".native-index-v18"))?;
    assert!(
        bind_service(
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
    std::fs::write(&target, b"quanta-index-native-state-v18\n")?;
    std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o600))?;
    std::fs::hard_link(&target, linked.path().join(".native-index-v18"))?;
    assert!(
        bind_service(
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
fn native_format_refuses_v10_before_reinterpreting_capture_claims() -> TestResult {
    let root = private_root()?;
    let marker = root.path().join(".native-index-v10");
    let original_marker = b"quanta-index-native-state-v10\n";
    std::fs::write(&marker, original_marker)?;
    std::fs::set_permissions(&marker, std::fs::Permissions::from_mode(0o600))?;
    let old_catalog = root.path().join("catalog");
    std::fs::create_dir(&old_catalog)?;
    let old_database = old_catalog.join("catalog-v1.sqlite");
    let original_database = b"old captured claim bytes must not be opened or migrated";
    std::fs::write(&old_database, original_database)?;
    let socket = root.path().join("native.sock");
    let error = match bind_service(root.path(), &socket, work_limits(), descriptors()) {
        Ok(_service) => return Err("v10 capture state unexpectedly opened".into()),
        Err(error) => error,
    };
    assert!(error.to_string().contains("explicit rebuild"));
    assert_eq!(std::fs::read(&marker)?, original_marker);
    assert_eq!(std::fs::read(&old_database)?, original_database);
    for entry in [
        ".native-index-v18",
        "native-objects",
        "native-scratch-v1",
        "native.sock",
    ] {
        assert!(!root.path().join(entry).exists());
    }
    Ok(())
}

#[test]
fn native_format_refuses_v11_before_reinterpreting_source_authority() -> TestResult {
    let root = private_root()?;
    let marker = root.path().join(".native-index-v11");
    let original_marker = b"quanta-index-native-state-v11\n";
    std::fs::write(&marker, original_marker)?;
    std::fs::set_permissions(&marker, std::fs::Permissions::from_mode(0o600))?;
    let old_catalog = root.path().join("catalog");
    std::fs::create_dir(&old_catalog)?;
    let old_database = old_catalog.join("catalog-v1.sqlite");
    let original_database = b"old source authority bytes must not be opened or migrated";
    std::fs::write(&old_database, original_database)?;
    let socket = root.path().join("native.sock");
    let error = match bind_service(root.path(), &socket, work_limits(), descriptors()) {
        Ok(_service) => return Err("v11 capture state unexpectedly opened".into()),
        Err(error) => error,
    };
    assert!(error.to_string().contains("explicit rebuild"));
    assert_eq!(std::fs::read(&marker)?, original_marker);
    assert_eq!(std::fs::read(&old_database)?, original_database);
    for entry in [
        ".native-index-v18",
        "native-objects",
        "native-scratch-v1",
        "native.sock",
    ] {
        assert!(!root.path().join(entry).exists());
    }
    Ok(())
}

#[test]
fn native_format_marker_matches_the_shared_persisted_state_version() {
    use quanta_index_contract::index::NATIVE_INDEX_STATE_VERSION;
    use quanta_index_searchd_runtime::native::{NATIVE_MARKER_CONTENT, NATIVE_MARKER_NAME};

    assert_eq!(
        NATIVE_MARKER_NAME,
        format!(".native-index-v{NATIVE_INDEX_STATE_VERSION}")
    );
    assert_eq!(
        NATIVE_MARKER_CONTENT,
        format!("quanta-index-native-state-v{NATIVE_INDEX_STATE_VERSION}\n").as_bytes()
    );
}

#[test]
fn native_format_refuses_v9_without_creating_retention_state() -> TestResult {
    let root = private_root()?;
    let original = b"quanta-index-native-state-v9\n";
    std::fs::write(root.path().join(".native-index-v9"), original)?;
    let socket = root.path().join("native.sock");
    let error = match bind_service(root.path(), &socket, work_limits(), descriptors()) {
        Ok(_service) => return Err("v9 native state unexpectedly opened".into()),
        Err(error) => error,
    };
    assert!(
        error
            .to_string()
            .contains("prior native index state requires an explicit rebuild")
    );
    assert_eq!(
        std::fs::read(root.path().join(".native-index-v9"))?,
        original
    );
    assert!(!root.path().join(".native-index-v18").exists());
    assert!(!root.path().join("catalog").exists());
    assert!(!socket.exists());
    Ok(())
}

#[test]
fn native_v12_root_refuses_without_marker_or_catalog_rewrite() -> TestResult {
    let root = private_root()?;
    let marker = root.path().join(".native-index-v12");
    std::fs::write(&marker, b"quanta-index-native-state-v12\n")?;
    std::fs::set_permissions(&marker, std::fs::Permissions::from_mode(0o600))?;
    let error = match bind_service(
        root.path(),
        &root.path().join("native.sock"),
        work_limits(),
        descriptors(),
    ) {
        Ok(_service) => return Err("v12 root was accepted without a nonce/rebuild".into()),
        Err(error) => error,
    };
    assert!(error.to_string().contains("explicit rebuild"));
    assert_eq!(std::fs::read(&marker)?, b"quanta-index-native-state-v12\n");
    assert!(!root.path().join(".native-index-v18").exists());
    assert!(!root.path().join("catalog").exists());
    assert!(!root.path().join("native-objects").exists());
    Ok(())
}

#[test]
fn native_v13_root_refuses_without_marker_or_catalog_rewrite() -> TestResult {
    let root = private_root()?;
    let marker = root.path().join(".native-index-v13");
    std::fs::write(&marker, b"quanta-index-native-state-v13\n")?;
    std::fs::set_permissions(&marker, std::fs::Permissions::from_mode(0o600))?;
    let error = match bind_service(
        root.path(),
        &root.path().join("native.sock"),
        work_limits(),
        descriptors(),
    ) {
        Ok(_service) => {
            return Err("v13 root was accepted without boot-custody format/rebuild".into());
        }
        Err(error) => error,
    };
    assert!(error.to_string().contains("explicit rebuild"));
    assert_eq!(std::fs::read(&marker)?, b"quanta-index-native-state-v13\n");
    assert!(!root.path().join(".native-index-v18").exists());
    assert!(!root.path().join("catalog").exists());
    assert!(!root.path().join("native-objects").exists());
    Ok(())
}

#[test]
fn native_v14_root_refuses_without_marker_or_catalog_rewrite() -> TestResult {
    let root = private_root()?;
    let marker = root.path().join(".native-index-v14");
    std::fs::write(&marker, b"quanta-index-native-state-v14\n")?;
    std::fs::set_permissions(&marker, std::fs::Permissions::from_mode(0o600))?;
    let error = match bind_service(
        root.path(),
        &root.path().join("native.sock"),
        work_limits(),
        descriptors(),
    ) {
        Ok(_service) => {
            return Err("v14 root was accepted without terminal-child format/rebuild".into());
        }
        Err(error) => error,
    };
    assert!(error.to_string().contains("explicit rebuild"));
    assert_eq!(std::fs::read(&marker)?, b"quanta-index-native-state-v14\n");
    assert!(!root.path().join(".native-index-v18").exists());
    assert!(!root.path().join("catalog").exists());
    assert!(!root.path().join("native-objects").exists());
    Ok(())
}

#[test]
fn native_v15_root_refuses_without_marker_or_catalog_rewrite() -> TestResult {
    let root = private_root()?;
    let marker = root.path().join(".native-index-v15");
    std::fs::write(&marker, b"quanta-index-native-state-v15\n")?;
    std::fs::set_permissions(&marker, std::fs::Permissions::from_mode(0o600))?;
    let error = match bind_service(
        root.path(),
        &root.path().join("native.sock"),
        work_limits(),
        descriptors(),
    ) {
        Ok(_service) => {
            return Err("v15 root was accepted without compact Text format/rebuild".into());
        }
        Err(error) => error,
    };
    assert!(error.to_string().contains("explicit rebuild"));
    assert_eq!(std::fs::read(&marker)?, b"quanta-index-native-state-v15\n");
    assert!(!root.path().join(".native-index-v18").exists());
    assert!(!root.path().join("catalog").exists());
    assert!(!root.path().join("native-objects").exists());
    Ok(())
}

#[test]
fn native_v16_root_refuses_without_marker_or_catalog_rewrite() -> TestResult {
    let root = private_root()?;
    let marker = root.path().join(".native-index-v16");
    std::fs::write(&marker, b"quanta-index-native-state-v16\n")?;
    std::fs::set_permissions(&marker, std::fs::Permissions::from_mode(0o600))?;
    let error = match bind_service(
        root.path(),
        &root.path().join("native.sock"),
        work_limits(),
        descriptors(),
    ) {
        Ok(_service) => {
            return Err("v16 root was accepted without bounded map-value format/rebuild".into());
        }
        Err(error) => error,
    };
    assert!(error.to_string().contains("explicit rebuild"));
    assert_eq!(std::fs::read(&marker)?, b"quanta-index-native-state-v16\n");
    assert!(!root.path().join(".native-index-v18").exists());
    assert!(!root.path().join("catalog").exists());
    assert!(!root.path().join("native-objects").exists());
    Ok(())
}

#[test]
fn native_v17_root_refuses_without_marker_or_catalog_rewrite() -> TestResult {
    let root = private_root()?;
    let marker = root.path().join(".native-index-v17");
    std::fs::write(&marker, b"quanta-index-native-state-v17\n")?;
    std::fs::set_permissions(&marker, std::fs::Permissions::from_mode(0o600))?;
    let error = match bind_service(
        root.path(),
        &root.path().join("native.sock"),
        work_limits(),
        descriptors(),
    ) {
        Ok(_service) => {
            return Err("v17 root was accepted without compound map-value format/rebuild".into());
        }
        Err(error) => error,
    };
    assert!(error.to_string().contains("explicit rebuild"));
    assert_eq!(std::fs::read(&marker)?, b"quanta-index-native-state-v17\n");
    assert!(!root.path().join(".native-index-v18").exists());
    assert!(!root.path().join("catalog").exists());
    assert!(!root.path().join("native-objects").exists());
    Ok(())
}

#[test]
fn native_automatic_retention_advances_floor_releases_custody_and_preserves_pin_and_replay()
-> TestResult {
    use quanta_index_core::index::ports::{
        CatalogCustodyMaintenancePort, CatalogExactHistoryPort, OperationJournalPort,
    };
    use quanta_index_searchd_runtime::native::{NativeRetentionConfig, NativeRetentionScope};

    let root = private_root()?;
    let socket = root.path().join("native.sock");
    let mut resources = test_resources();
    resources.execution.capacity.cpu_units = 5;
    resources.execution.capacity.io_bytes = 320 * 1024 * 1024;
    resources.execution.capacity.memory_bytes = (5 + 1) * 64 * 1024 * 1024;
    // This law covers pin/history/replay correctness for two complete COW
    // source transitions, not a one-object-per-round throughput guarantee.
    // Four steps fit the unchanged 512 MiB WorkLimits (128 MiB per step).
    // Single-step cursor/partial-drain laws retain their separate coverage.
    let reclaim = NativeReclaimConfig {
        max_steps_per_round: 4,
        ..test_reclaim_config()
    };
    reclaim.validate()?;
    let retention = NativeRetentionConfig {
        scopes: vec![NativeRetentionScope {
            workspace: workspace(),
            epoch: 1,
        }],
        sequence_window: 1,
        // This fixture publishes two literal sources, with many immutable
        // object custody rows. Keep the 10s watchdog and fund eight rows per
        // custody phase; Catalog and the product fixture retain single-row
        // partial/restart coverage separately.
        page_limit: 8,
        interval: Duration::from_millis(20),
        round_timeout: Duration::from_secs(2),
        work_limits: work_limits(),
        head_probe_every: 2,
    };
    let mut capabilities = reclaim_capabilities(None)?;
    let path_need = ResourceNeed {
        provider_units: 0,
        ..resources.execution.interactive
    };
    capabilities.path.register(
        descriptors().file_map,
        path_need,
        NativePathReader::new(path_need)?,
    )?;
    let service = NativeIndexService::bind_with_retention(
        root.path(),
        &socket,
        work_limits(),
        capabilities,
        resources,
        Some(reclaim),
        Some(retention.clone()),
    )?;
    let running = NativeServiceGuard::start(service);
    let client = NativeUdsClient::new(socket.clone());
    let reply = client.request(
        NativeIndexRequest::CreateWorkspace {
            workspace: workspace(),
            epoch: 1,
        },
        &budget(),
    )?;
    let NativeIndexResponse::Created(genesis) = reply else {
        return Err("missing retention genesis".into());
    };
    let ingress = client.begin_input(operation(1), &budget())?;
    let profile = client.stage_profile(
        ingress,
        IndexProfile {
            instances: vec![literal_spec(), file_spec()],
        },
        &budget(),
    )?;
    let original = UpdateRequest {
        operation: operation(1),
        expected_view: genesis,
        kind: UpdateKind::Reconfigure { profile },
    };
    let configured = published(&client.submit(original.clone(), &budget())?)?;
    let (input, record) =
        stage_source(&client, operation(2), "first.rs", b"first retained source")?;
    let two = advance_one(&client, input, configured, owner(1), record, 1)?;
    let record_file = native_object_file(root.path(), record)?;
    let original_record_wire = std::fs::read(&record_file)?;
    running.stop()?;
    // Establish the independent live pin before admitted boot/automatic
    // recovery begins; never initialize a second raw writer on its live root.
    let catalog = SqliteCatalog::open(root.path(), Duration::from_secs(2))?;
    let catalog_path = catalog.path().to_path_buf();
    let session = catalog.open_read_session()?;
    let pin = catalog.acquire_ticket(
        session.as_ref(),
        ViewSelection::Exact(two),
        Duration::from_secs(30),
        &WorkBudget::with_request(work_limits(), budget()),
    )?;
    let mut capabilities = reclaim_capabilities(None)?;
    capabilities.path.register(
        descriptors().file_map,
        path_need,
        NativePathReader::new(path_need)?,
    )?;
    let running = NativeServiceGuard::start(
        NativeIndexService::bind_with_retention(
            root.path(),
            &socket,
            work_limits(),
            capabilities,
            resources,
            Some(reclaim),
            Some(retention.clone()),
        )
        .map_err(|error| format!("retention restart with independent live pin: {error:#}"))?,
    );
    let old_paths = client.query_paths(
        ViewSelection::Exact(two),
        file_spec().instance,
        "",
        None,
        8,
        &budget(),
    )?;
    assert_eq!(old_paths.view, two);
    assert_eq!(old_paths.entries.len(), 1);
    let old_entry = old_paths.entries.first().ok_or("old path entry absent")?;
    assert_eq!(old_entry.owner, owner(1));
    assert_eq!(old_entry.path, "first.rs");
    assert_eq!(old_entry.source, record);
    let old_body = old_entry.bytes;
    let body_file = native_object_file(root.path(), old_body)?;
    let original_body_wire = std::fs::read(&body_file)?;
    let (input, second_record) =
        stage_source(&client, operation(3), "second.rs", b"second source")?;
    let three = advance_one(&client, input, two, owner(2), second_record, 2)?;
    // Retract the first owner so the old record is reachable only through
    // pinned history once other below-floor views and terminal custody drain.
    let input = client.begin_input(operation(4), &budget())?;
    let prior = client.read_view(ViewSelection::Exact(three), &budget())?;
    let owners = client.stage_mutations(
        input,
        [OwnerMutation {
            owner: owner(1),
            expected: Some(record),
            next: None,
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
        predecessor: 2,
        sequence: 3,
        payload: transition.owner_digest()?,
    });
    let four = published(&client.submit(
        UpdateRequest {
            operation: operation(4),
            expected_view: three,
            kind: UpdateKind::SourceAdvance(transition),
        },
        &budget(),
    )?)?;
    let connection = rusqlite::Connection::open_with_flags(
        catalog.path(),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?;
    let current_sequence_sql = i64::try_from(four.sequence)?;
    let pinned_sequence_sql = i64::try_from(two.sequence)?;
    let deadline = Instant::now()
        .checked_add(Duration::from_secs(10))
        .ok_or("retention deadline overflow")?;
    loop {
        let observed_floor = catalog.exact_history_floor(workspace(), 1)?;
        let next_custody = catalog.next_terminal_custody(
            workspace(),
            1,
            None,
            None,
            &mut WorkBudget::with_request(work_limits(), budget()),
        )?;
        let other_history: i64 = connection.query_row(
            "SELECT COUNT(*) FROM native_view_v1 WHERE sequence<?1 AND sequence!=?2",
            rusqlite::params![current_sequence_sql, pinned_sequence_sql],
            |row| row.get(0),
        )?;
        if observed_floor == four.sequence && next_custody.is_none() && other_history == 0 {
            break;
        }
        if Instant::now() >= deadline {
            let worker = running.observation.observe(Duration::from_secs(1));
            let resources = running.resources.observe();
            let shutdown = running.stop();
            return Err(format!("automatic retention failed to settle floor/custody: expected_floor={}, observed_floor={observed_floor}, next_custody={next_custody:?}, other_history={other_history}, worker={worker:?}, resources={resources:?}, shutdown={shutdown:?}", four.sequence).into());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    drop(connection);
    assert_eq!(pin.view.identity, two);
    assert_eq!(std::fs::read(&record_file)?, original_record_wire);
    assert_eq!(std::fs::read(&body_file)?, original_body_wire);
    assert_eq!(
        catalog.inspect(
            original.operation,
            quanta_index_core::index::ports::OperationInspectAccess::Request(
                &WorkBudget::with_request(work_limits(), budget()),
            ),
        )?,
        OperationStatus::Published(configured)
    );
    assert_eq!(published(&client.submit(original, &budget())?)?, configured);
    assert!(matches!(
        client.query_paths(ViewSelection::Exact(two), file_spec().instance, "", None, 8, &budget()),
        Err(NativeUdsError::Refused(error)) if error.code == NativeIndexErrorCode::SnapshotUnavailable
    ));
    // A raw observer can share SQLite's underlying SHM descriptor. Close it
    // before requesting the admitted owner's physical retirement witness.
    drop(catalog);
    running.stop()?;
    // Validation advances the Catalog work clock, so it is also a raw write.
    // Keep it outside the live admitted service alongside explicit release.
    let catalog = SqliteCatalog::open(root.path(), Duration::from_secs(2))?;
    catalog.validate_ticket(session.as_ref(), pin.lease)?;
    catalog.release_ticket(session.as_ref(), pin.lease)?;
    drop(session);
    drop(catalog);
    let mut capabilities = reclaim_capabilities(None)?;
    capabilities.path.register(
        descriptors().file_map,
        path_need,
        NativePathReader::new(path_need)?,
    )?;
    let service = NativeIndexService::bind_with_retention(
        root.path(),
        &socket,
        work_limits(),
        capabilities,
        resources,
        Some(reclaim),
        Some(retention),
    )
    .map_err(|error| format!("retention restart after raw pin release: {error:#}"))?;
    let maintenance = service.maintenance_handle();
    let running = NativeServiceGuard::start(service);
    let deadline = Instant::now()
        .checked_add(Duration::from_secs(10))
        .ok_or("record reclaim deadline overflow")?;
    while !native_objects_absent_nofollow(root.path(), &[record, old_body])?
        && Instant::now() < deadline
    {
        std::thread::sleep(Duration::from_millis(20));
    }
    if !native_objects_absent_nofollow(root.path(), &[record, old_body])? {
        let connection = rusqlite::Connection::open_with_flags(
            &catalog_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )?;
        let queued: rusqlite::Result<Vec<String>> = connection
            .prepare("SELECT hex(object) FROM native_gc_queue_v1 ORDER BY object LIMIT 8")?
            .query_map([], |row| row.get(0))?
            .collect();
        let queue_count: rusqlite::Result<i64> =
            connection.query_row("SELECT COUNT(*) FROM native_gc_queue_v1", [], |row| {
                row.get(0)
            });
        let before_body: Option<Vec<u8>> = connection.query_row(
            "SELECT MAX(object) FROM native_gc_queue_v1 WHERE object<?1",
            [old_body.0.0.as_slice()],
            |row| row.get(0),
        )?;
        let before_body = before_body
            .map(|bytes| <[u8; 32]>::try_from(bytes).map(|bytes| ObjectRef(Digest(bytes))))
            .transpose()
            .map_err(|_bytes| "diagnostic queue predecessor is not an ObjectRef")?;
        let history_and_pins: rusqlite::Result<(i64, i64, i64)> = connection.query_row(
            "SELECT (SELECT COUNT(*) FROM native_view_v1 WHERE sequence=?1),
                    (SELECT COUNT(*) FROM native_pin_v1 WHERE sequence=?1),
                    (SELECT COUNT(*) FROM native_work_v1 WHERE sequence=?1)",
            [pinned_sequence_sql],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        );
        let mut targets = Vec::new();
        for object in [record, old_body] {
            let lifecycle: rusqlite::Result<(i64, i64)> = connection.query_row(
                "SELECT COALESCE((SELECT phase FROM native_gc_object_v1 WHERE object=?1),-1),
                 (SELECT COUNT(*) FROM native_gc_queue_v1 WHERE object=?1)",
                [object.0.0.as_slice()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            );
            let mut references = Vec::new();
            // Mirror the complete authority reference set only for failure
            // diagnosis; never use these counts as a retirement permission.
            for (table, column) in [
                ("native_view_root_v1", "object"),
                ("native_operation_object_v1", "object"),
                ("native_ingress_intent_v1", "object"),
                ("native_ingress_object_v1", "object"),
                ("native_attempt_object_v1", "object"),
                ("native_object_intent_v1", "object"),
                ("native_work_object_v1", "object"),
                ("native_bootstrap_object_v1", "object"),
                ("native_bootstrap_v1", "profile"),
                ("native_object_edge_v1", "child"),
                ("native_object_install_edge_v1", "child"),
            ] {
                let count: rusqlite::Result<i64> = connection.query_row(
                    &format!("SELECT COUNT(*) FROM {table} WHERE {column}=?1"),
                    [object.0.0.as_slice()],
                    |row| row.get(0),
                );
                references.push((table, count));
            }
            let mut parents = Vec::new();
            for table in ["native_object_edge_v1", "native_object_install_edge_v1"] {
                let observed = (|| -> rusqlite::Result<Vec<_>> {
                    let mut statement = connection.prepare(&format!(
                        "SELECT hex(substr(e.parent,1,32)),g.phase,
                          EXISTS(SELECT 1 FROM native_gc_queue_v1 q WHERE q.object=e.parent),
                          (SELECT COUNT(*) FROM native_view_root_v1 v WHERE v.object=e.parent)
                         FROM {table} e LEFT JOIN native_gc_object_v1 g ON g.object=e.parent
                         WHERE e.child=?1 ORDER BY e.parent LIMIT 8"
                    ))?;
                    statement
                        .query_map([object.0.0.as_slice()], |row| {
                            Ok((
                                row.get::<_, String>(0)?,
                                row.get::<_, Option<i64>>(1)?,
                                row.get::<_, bool>(2)?,
                                row.get::<_, i64>(3)?,
                            ))
                        })?
                        .collect()
                })();
                parents.push((table, observed));
            }
            targets.push((object, lifecycle, references, parents));
        }
        drop(connection);
        let worker = running.observation.observe(Duration::from_secs(1));
        let resources = running.resources.observe();
        let shutdown = running.stop();
        // Failure-only probe after automatic work has joined. The original
        // 10s automatic oracle remains failed even if this manual step works.
        // Reuse the admitted owner and original per-round limits/deadline;
        // never open an unadmitted Catalog or raise capacity for diagnosis.
        let config = reclaim;
        let mut probe_budget = WorkBudget::with_request(
            config.work_limits,
            RequestBudgetV1::for_duration(config.round_timeout),
        );
        let probe_started = Instant::now();
        let queue_probe =
            maintenance.reclaim_step(before_body, config.child_limit, &mut probe_budget);
        let probe_elapsed = probe_started.elapsed();
        let probe_usage = probe_budget.usage();
        return Err(format!(
            "automatic physical retention did not remove old record/body: targets(object,lifecycle,all_reference_counts,parent_prefixes)={targets:?}, pinned_history_views_pins_work={history_and_pins:?}, queue_count={queue_count:?}, queue_first8={queued:?}, worker={worker:?}, resources={resources:?}, shutdown={shutdown:?}, admitted_queue_probe={queue_probe:?}, probe_elapsed={probe_elapsed:?}, probe_usage={probe_usage:?}"
        ).into());
    }
    assert_eq!(
        client
            .read_view(ViewSelection::Exact(four), &budget())?
            .identity,
        four
    );
    let current_paths = client.query_paths(
        ViewSelection::Exact(four),
        file_spec().instance,
        "",
        None,
        8,
        &budget(),
    )?;
    assert_eq!(current_paths.view, four);
    assert_eq!(current_paths.entries.len(), 1);
    let current_entry = current_paths
        .entries
        .first()
        .ok_or("current path entry absent")?;
    assert_eq!(current_entry.path, "second.rs");
    assert_eq!(current_entry.owner, owner(2));
    assert_eq!(current_entry.source, second_record);
    assert!(!current_paths.has_more);
    drop(maintenance);
    running.stop()?;
    let reopened = bind_service(root.path(), &socket, work_limits(), descriptors())?;
    drop(reopened);
    Ok(())
}

#[test]
fn native_bind_rejects_unusable_read_lease_before_format_creation() -> TestResult {
    for lease in [Duration::from_nanos(1), Duration::from_secs(301)] {
        let root = private_root()?;
        let error = NativeIndexService::bind(
            root.path(),
            &root.path().join("native.sock"),
            work_limits(),
            descriptors(),
            NativeServiceResources {
                physical: native_physical_profile::profile(),
                scratch_reserved_bytes: 16 * 1024 * 1024 * 1024,
                read_ticket_lease: lease,
                execution: test_resources().execution,
            },
        );
        assert!(error.is_err());
        assert!(!root.path().join(".native-index-v18").exists());
        assert!(!root.path().join("catalog").exists());
    }
    Ok(())
}

type NativeRegularFiles = std::collections::BTreeMap<std::path::PathBuf, Vec<u8>>;

fn native_regular_files(root: &std::path::Path) -> TestResult<NativeRegularFiles> {
    use std::os::unix::fs::FileTypeExt as _;
    let mut result = NativeRegularFiles::new();
    let mut directories = vec![root.to_path_buf()];
    while let Some(directory) = directories.pop() {
        for entry in std::fs::read_dir(directory)? {
            let entry = entry?;
            let metadata = std::fs::symlink_metadata(entry.path())?;
            if metadata.is_dir() {
                directories.push(entry.path());
            } else if metadata.is_file() {
                let previous = result.insert(
                    entry.path().strip_prefix(root)?.to_path_buf(),
                    std::fs::read(entry.path())?,
                );
                assert!(previous.is_none());
            } else {
                assert!(metadata.file_type().is_socket());
            }
        }
    }
    Ok(result)
}

fn native_regular_file_bytes(files: &NativeRegularFiles) -> TestResult<u64> {
    files
        .values()
        .try_fold(0_u64, |sum, data| -> TestResult<u64> {
            sum.checked_add(u64::try_from(data.len())?)
                .ok_or_else(|| std::io::Error::other("fixture census overflow").into())
        })
}

fn check_native_physical_census(
    service: &NativeIndexService,
    root: &std::path::Path,
) -> TestResult {
    let observed = native_regular_files(root)?;
    let bytes = native_regular_file_bytes(&observed)?;
    let charged = service.physical_observation()?;
    let (reserve_bytes, reserve_files) = service.maintenance_reserve_available()?;
    assert_eq!(
        charged.used_bytes,
        bytes
            .checked_add(reserve_bytes)
            .ok_or("fixture byte census and reserve overflow")?
    );
    assert_eq!(
        charged.used_files,
        u64::try_from(observed.len())?
            .checked_add(reserve_files)
            .ok_or("fixture file census and reserve overflow")?
    );
    assert_eq!(charged.active_allowances, 1);
    assert_eq!(charged.active_tickets, 0);
    assert_eq!(charged.uncertain_pending, 0);
    assert!(!charged.poisoned);
    Ok(())
}

#[test]
fn native_physical_boot_accounts_actual_files_and_reopens() -> TestResult {
    let root = private_root()?;
    let socket = root.path().join("native.sock");
    let service = bind_service(root.path(), &socket, work_limits(), descriptors())?;
    check_native_physical_census(&service, root.path())?;
    drop(service);
    let reopened = bind_service(root.path(), &socket, work_limits(), descriptors())?;
    check_native_physical_census(&reopened, root.path())?;
    drop(reopened);
    Ok(())
}

fn native_physical_staging_pressure(byte_cap: bool, raise_cap: bool) -> TestResult {
    let root = private_root()?;
    let socket = root.path().join("native.sock");
    let running = NativeServiceGuard::start(bind_service(
        root.path(),
        &socket,
        work_limits(),
        descriptors(),
    )?);
    let client = NativeUdsClient::new(socket.clone());
    let created = client.request(
        NativeIndexRequest::CreateWorkspace {
            workspace: workspace(),
            epoch: 1,
        },
        &budget(),
    )?;
    let NativeIndexResponse::Created(genesis) = created else {
        return Err(format!("pressure fixture genesis: {created:?}").into());
    };
    running.stop()?;
    let baseline = native_regular_files(root.path())?;
    let mut resources = test_resources();
    if byte_cap {
        let baseline_bytes = native_regular_file_bytes(&baseline)?;
        resources.physical.inventory.max_bytes = baseline_bytes
            .checked_add(2 * 1024 * 1024)
            .ok_or("physical fixture byte cap overflow")?;
    } else {
        resources.physical.inventory.max_files = u64::try_from(baseline.len())?
            .checked_add(20)
            .ok_or("physical fixture file cap overflow")?;
    }
    let limits = resources.physical.inventory;
    let service = NativeIndexService::bind(
        root.path(),
        &socket,
        work_limits(),
        descriptors(),
        resources,
    )?;
    check_native_physical_census(&service, root.path())?;
    let running = NativeServiceGuard::start(service);
    let ingress = client.begin_input(operation(7000), &budget())?;
    let seed = client.stage_sources(
        ingress,
        vec![SourceInput {
            path: "pressure-seed.rs".into(),
            bytes: b"physical admission seed".to_vec(),
            text: SourceTextAdmission::Excluded,
        }],
        &budget(),
    )?;
    let seed = *seed.first().ok_or("pressure seed absent")?;
    // Exercise the actual map scratch path before filling the shared cap.
    let mutations = client.stage_mutations(
        ingress,
        [OwnerMutation {
            owner: OwnerKey {
                producer: ingress.operation.producer,
                stable_id: StableOwnerId([71; 16]),
            },
            expected: None,
            next: Some(seed),
        }],
        &budget(),
    )?;
    assert_eq!(mutations.count, 1);
    let mut accepted = 0_u32;
    let mut refused = false;
    for index in 0_u8..32 {
        let result = client.stage_sources(
            ingress,
            vec![SourceInput {
                path: format!("pressure-{index}.rs"),
                bytes: vec![index; 128 * 1024],
                text: SourceTextAdmission::Excluded,
            }],
            &budget(),
        );
        let files = native_regular_files(root.path())?;
        let bytes = native_regular_file_bytes(&files)?;
        assert!(bytes <= limits.max_bytes, "live regular-file bytes {bytes}");
        assert!(u64::try_from(files.len())? <= limits.max_files);
        match result {
            Ok(staged) => {
                assert_eq!(staged.len(), 1);
                accepted = accepted
                    .checked_add(1)
                    .ok_or("accepted physical fixture count overflow")?;
            }
            Err(NativeUdsError::Refused(error))
                if error.code == NativeIndexErrorCode::ResourceLimited =>
            {
                refused = true;
                break;
            }
            Err(error) => {
                return Err(format!("physical pressure stage {index}: {error:?}").into());
            }
        }
    }
    assert!(accepted > 0, "no full-size source was accepted");
    assert!(refused, "declared physical cap was not reached");
    if !raise_cap {
        // Cleanup writes use the pre-held reserve while foreground is refused.
        client
            .discard_input(ingress, &budget())
            .map_err(|error| format!("same-cap initial discard: {error:?}"))?;
    }
    running.stop()?;

    // The cap-raised laws preserve the original ingress. Reserve laws recover
    // the discarded ingress under the exact same physical limits.
    let recovery_resources = if raise_cap {
        test_resources()
    } else {
        resources
    };
    let reopened = NativeIndexService::bind(
        root.path(),
        &socket,
        work_limits(),
        descriptors(),
        recovery_resources,
    )
    .map_err(|error| format!("pressure first recovery bind: {error:#}"))?;
    check_native_physical_census(&reopened, root.path())?;
    let maintenance = reopened.maintenance_handle();
    let running = NativeServiceGuard::start(reopened);
    if !raise_cap {
        let mut after = None;
        let mut drained = false;
        for step_index in 0..128 {
            let mut work = WorkBudget::with_request(work_limits(), budget());
            let step = maintenance
                .reclaim_step(after, 32, &mut work)
                .map_err(|error| {
                    format!("same-cap reclaim step {step_index}, cursor {after:?}: {error:?}")
                })?;
            after = step.resume_after;
            if !step.has_more {
                drained = true;
                break;
            }
        }
        assert!(drained, "bounded discarded-input GC did not finish");
        assert_eq!(
            client
                .read_view(ViewSelection::Exact(genesis), &budget())
                .map_err(|error| format!("same-cap ordinary read after GC: {error:?}"))?
                .identity,
            genesis
        );
        let next_ingress = client
            .begin_input(operation(7040), &budget())
            .map_err(|error| format!("same-cap ordinary begin after GC: {error:?}"))?;
        let staged = client
            .stage_sources(
                next_ingress,
                vec![SourceInput {
                    path: "after-pressure.rs".into(),
                    bytes: b"ordinary admission after bounded GC".to_vec(),
                    text: SourceTextAdmission::Excluded,
                }],
                &budget(),
            )
            .map_err(|error| format!("same-cap ordinary stage after GC: {error:?}"))?;
        assert_eq!(staged.len(), 1);
        client
            .discard_input(next_ingress, &budget())
            .map_err(|error| format!("same-cap subsequent discard: {error:?}"))?;
        running.stop()?;
        drop(maintenance);
        let reopened = NativeIndexService::bind(
            root.path(),
            &socket,
            work_limits(),
            descriptors(),
            resources,
        )
        .map_err(|error| format!("same-cap final recovery bind: {error:#}"))?;
        check_native_physical_census(&reopened, root.path())?;
        drop(reopened);
        return Ok(());
    }
    // Reuse the existing ingress before explicitly requesting a new fence.
    assert_eq!(
        client.stage_sources(
            ingress,
            vec![SourceInput {
                path: "pressure-seed.rs".into(),
                bytes: b"physical admission seed".to_vec(),
                text: SourceTextAdmission::Excluded,
            }],
            &budget(),
        )?,
        vec![seed]
    );
    assert_eq!(
        client
            .read_view(ViewSelection::Exact(genesis), &budget())?
            .identity,
        genesis
    );
    let refenced = client.begin_input(ingress.operation, &budget())?;
    assert_eq!(refenced.operation, ingress.operation);
    assert!(refenced.fence > ingress.fence);
    assert!(matches!(
        client.discard_input(ingress, &budget()),
        Err(NativeUdsError::Refused(error)) if error.code == NativeIndexErrorCode::FenceLost
    ));
    client.discard_input(refenced, &budget())?;
    running.stop()?;
    drop(maintenance);
    let reopened = bind_service(root.path(), &socket, work_limits(), descriptors())?;
    check_native_physical_census(&reopened, root.path())?;
    drop(reopened);
    Ok(())
}

#[test]
fn native_physical_staging_refuses_at_live_byte_cap_and_recovers() -> TestResult {
    native_physical_staging_pressure(true, true)
}

#[test]
fn native_physical_staging_refuses_at_live_file_cap_and_recovers() -> TestResult {
    native_physical_staging_pressure(false, true)
}

#[test]
fn native_physical_byte_reserve_discards_and_reclaims_under_the_same_cap() -> TestResult {
    native_physical_staging_pressure(true, false)
}

#[test]
fn native_physical_file_reserve_discards_and_reclaims_under_the_same_cap() -> TestResult {
    native_physical_staging_pressure(false, false)
}

#[test]
fn native_physical_repeated_boot_retires_dead_sessions_under_the_same_file_cap() -> TestResult {
    let root = private_root()?;
    let socket = root.path().join("native.sock");
    let initial = bind_service(root.path(), &socket, work_limits(), descriptors())?;
    check_native_physical_census(&initial, root.path())?;
    drop(initial);
    let baseline = native_regular_files(root.path())?;
    let mut resources = test_resources();
    resources.physical.inventory.max_files = u64::try_from(baseline.len())?
        .checked_add(u64::from(resources.physical.maintenance_reserved_files))
        .and_then(|count| count.checked_add(2))
        .ok_or("repeated boot file cap overflow")?;
    // More restarts than the whole spare inode window; no external cleanup,
    // no foreground operation to refresh headroom, and no operator cap change.
    for restart in 0..32 {
        let service = NativeIndexService::bind(
            root.path(),
            &socket,
            work_limits(),
            descriptors(),
            resources,
        )
        .map_err(|error| format!("same-cap repeated boot {restart}: {error:#}"))?;
        check_native_physical_census(&service, root.path())?;
        drop(service);
        let closed = native_regular_files(root.path())?;
        assert_eq!(closed.len(), baseline.len(), "dead session inode growth");
    }
    Ok(())
}

#[test]
fn native_physical_automatic_session_recovery_drains_backlog_and_preserves_live_lock() -> TestResult
{
    let root = private_root()?;
    let socket = root.path().join("native.sock");
    drop(bind_service(
        root.path(),
        &socket,
        work_limits(),
        descriptors(),
    )?);
    // Set up real old Catalog sessions before the managed service is present.
    // The live OS lock is the independent negative oracle, not age or PID.
    let catalog = SqliteCatalog::open(root.path(), Duration::from_secs(2))?;
    let sessions = root.path().join("catalog/native-read-sessions");
    let live = catalog.open_read_session()?;
    let live_path = sessions.join(format!("{:016x}.lease", live.binding().id));
    let mut dead_paths = Vec::new();
    for _ in 0..100 {
        let dead = catalog.open_read_session()?;
        dead_paths.push(sessions.join(format!("{:016x}.lease", dead.binding().id)));
        drop(dead);
    }
    drop(catalog);
    assert!(dead_paths.iter().all(|path| path.is_file()));
    let mut resources = test_resources();
    resources.execution.capacity.cpu_units = 5;
    resources.execution.capacity.io_bytes = 320 * 1024 * 1024;
    resources.execution.capacity.memory_bytes = (5 + 1) * 64 * 1024 * 1024;
    let service = NativeIndexService::bind_with_capabilities_and_maintenance(
        root.path(),
        &socket,
        work_limits(),
        reclaim_capabilities(None)?,
        resources,
        Some(test_reclaim_config()),
    )?;
    let running = NativeServiceGuard::start(service);
    // The boot page cannot drain 100 sessions. No manual recovery call occurs.
    let deadline = Instant::now()
        .checked_add(Duration::from_secs(10))
        .ok_or("session recovery deadline overflow")?;
    while dead_paths.iter().any(|path| path.exists()) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        dead_paths.iter().all(|path| !path.exists()),
        "dead session backlog retained"
    );
    assert!(live_path.is_file(), "live session lock was retired");
    drop(live);
    let deadline = Instant::now()
        .checked_add(Duration::from_secs(10))
        .ok_or("later session recovery deadline overflow")?;
    while live_path.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        !live_path.exists(),
        "completed sweep did not revisit a later-dead session"
    );
    running.stop()
}

#[test]
fn native_physical_invalid_profile_refuses_before_root_mutation() -> TestResult {
    for variant in 0..4 {
        let root = private_root()?;
        let mut resources = test_resources();
        match variant {
            0 => resources.physical.inventory.max_bytes = 0,
            1 => resources.physical.inventory.max_depth = 2,
            2 => resources.physical.max_reader_handles = 0,
            _ => resources.physical.global_max_sqlite_handles = 1,
        }
        let refused = NativeIndexService::bind(
            root.path(),
            &root.path().join("native.sock"),
            work_limits(),
            descriptors(),
            resources,
        );
        assert!(refused.is_err());
        assert!(std::fs::read_dir(root.path())?.next().is_none());
    }
    Ok(())
}

#[test]
fn native_physical_over_cap_reopen_preserves_existing_state() -> TestResult {
    let root = private_root()?;
    let socket = root.path().join("native.sock");
    drop(bind_service(
        root.path(),
        &socket,
        work_limits(),
        descriptors(),
    )?);
    // Include a valid immutable object in the byte-preservation gold.
    let scratch = Arc::new(FileScratchSpace::open_standalone(
        root.path().join("native-scratch-v1"),
        test_resources().scratch_reserved_bytes,
    )?);
    let objects = FileObjectStore::open_with_scratch(root.path().join("native-objects"), scratch)?;
    let _object = objects.put(
        b"preserve this existing object",
        &[],
        &mut WorkBudget::with_request(work_limits(), budget()),
    )?;
    drop(objects);
    let before = native_regular_files(root.path())?;
    for byte_cap in [true, false] {
        let mut resources = test_resources();
        if byte_cap {
            resources.physical.inventory.max_bytes =
                resources.physical.maintenance_reserved_bytes + 1;
        } else {
            resources.physical.inventory.max_files =
                u64::from(resources.physical.maintenance_reserved_files) + 1;
        }
        let error = NativeIndexService::bind(
            root.path(),
            &socket,
            work_limits(),
            descriptors(),
            resources,
        )
        .err()
        .ok_or("over-cap root was admitted")?;
        assert!(matches!(
            error.downcast_ref::<quanta_index_core::CoreError>(),
            Some(quanta_index_core::CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::IngestResourceBudgetExceeded,
                ..
            })
        ));
        assert_eq!(native_regular_files(root.path())?, before);
    }
    Ok(())
}

#[test]
fn native_bind_rejects_overcommitted_class_floors_before_format_creation() -> TestResult {
    let root = private_root()?;
    let mut resources = test_resources();
    resources.execution.capacity.cpu_units = 3;
    let result = NativeIndexService::bind(
        root.path(),
        &root.path().join("native.sock"),
        work_limits(),
        descriptors(),
        resources,
    );
    assert!(result.is_err());
    assert!(!root.path().join(".native-index-v18").exists());
    assert!(!root.path().join("catalog").exists());
    Ok(())
}

#[test]
fn native_reopen_accepts_owned_stale_socket_through_root_alias() -> TestResult {
    let root = private_root()?;
    let socket = root.path().join("native.sock");
    let initial = bind_service(root.path(), &socket, work_limits(), descriptors())?;
    drop(initial);

    let stale = UnixListener::bind(&socket)?;
    drop(stale);
    let alias_parent = private_root()?;
    let alias = alias_parent.path().join("root-alias");
    std::os::unix::fs::symlink(root.path(), &alias)?;
    let reopened = bind_service(
        &alias,
        &alias.join("native.sock"),
        work_limits(),
        descriptors(),
    )?;
    drop(reopened);
    Ok(())
}

#[test]
#[ignore = "invoked only by native_kill_reopen_retains_view_and_reclaims_owned_scratch"]
fn native_forced_exit_child() -> TestResult {
    let root =
        std::env::var_os("QIDX_NATIVE_FORCED_EXIT_ROOT").ok_or("forced-exit child root absent")?;
    let root = std::path::PathBuf::from(root);
    let ready = std::env::var_os("QIDX_NATIVE_FORCED_EXIT_READY")
        .ok_or("forced-exit child ready path absent")?;
    let pin_ready = std::env::var_os("QIDX_NATIVE_FORCED_EXIT_PIN_READY")
        .ok_or("forced-exit child pin ready path absent")?;
    // The unqualified fixture connection must finish all SQLite writes and
    // create its lock before the service seals the physical boot inventory.
    let catalog = SqliteCatalog::open(&root, Duration::from_millis(100))?;
    let session = catalog.open_read_session()?;
    let _ticket = catalog.acquire_ticket(
        session.as_ref(),
        ViewSelection::Latest {
            workspace: workspace(),
            epoch: 1,
        },
        Duration::from_secs(30),
        &WorkBudget::with_request(work_limits(), budget()),
    )?;
    let session_id = session.binding().id;
    drop(catalog);
    let service = bind_service(
        &root,
        &root.join("native.sock"),
        work_limits(),
        descriptors(),
    )?;
    let _server = std::thread::spawn(move || service.run(Duration::from_millis(5)));
    std::fs::write(pin_ready, session_id.to_string())?;
    std::fs::write(ready, b"ready")?;
    loop {
        std::thread::park();
    }
}

#[test]
fn native_kill_reopen_retains_view_and_reclaims_owned_scratch() -> TestResult {
    let root = private_root()?;
    let ready_directory = tempfile::tempdir()?;
    let ready = ready_directory.path().join("ready");
    let pin_ready = ready_directory.path().join("pin-ready");
    let socket = root.path().join("native.sock");
    let setup = NativeServiceGuard::start(bind_service(
        root.path(),
        &socket,
        work_limits(),
        descriptors(),
    )?);
    let client = NativeUdsClient::new(socket.clone());
    let genesis_reply = client.request(
        NativeIndexRequest::CreateWorkspace {
            workspace: workspace(),
            epoch: 1,
        },
        &budget(),
    )?;
    let NativeIndexResponse::Created(genesis) = genesis_reply else {
        return Err(format!("unexpected genesis reply: {genesis_reply:?}").into());
    };
    assert_eq!(
        client
            .read_view(ViewSelection::Exact(genesis), &budget())?
            .identity,
        genesis
    );
    setup.stop()?;
    let mut child = ForcedExitChild(
        std::process::Command::new(std::env::current_exe()?)
            .args([
                "--ignored",
                "--exact",
                "native_forced_exit_child",
                "--nocapture",
            ])
            .env("QIDX_NATIVE_FORCED_EXIT_ROOT", root.path())
            .env("QIDX_NATIVE_FORCED_EXIT_READY", &ready)
            .env("QIDX_NATIVE_FORCED_EXIT_PIN_READY", &pin_ready)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()?,
    );
    let proof = (|| -> TestResult<u64> {
        let deadline = Instant::now()
            .checked_add(Duration::from_secs(10))
            .ok_or("parent deadline overflow")?;
        while !ready.exists() {
            if let Some(status) = child.0.try_wait()? {
                return Err(format!("native child exited before bind: {status}").into());
            }
            if Instant::now() >= deadline {
                return Err("native child bind timed out".into());
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(bind_service(root.path(), &socket, work_limits(), descriptors()).is_err());
        let client = NativeUdsClient::new(socket.clone());
        assert_eq!(
            client
                .read_view(ViewSelection::Exact(genesis), &budget())?
                .identity,
            genesis
        );
        while !pin_ready.exists() {
            if let Some(status) = child.0.try_wait()? {
                return Err(format!("native child exited before read pin: {status}").into());
            }
            if Instant::now() >= deadline {
                return Err("native child read pin timed out".into());
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        Ok(std::fs::read_to_string(&pin_ready)?.parse()?)
    })();
    child.0.kill()?;
    let status = child.0.wait()?;
    assert!(!status.success());
    let dead_session_id = proof?;
    let session_path = |id: u64| {
        root.path()
            .join(format!("catalog/native-read-sessions/{id:016x}.lease"))
    };
    let session_counts = |id: u64| -> TestResult<(i64, i64)> {
        let connection = rusqlite::Connection::open_with_flags(
            root.path().join("catalog/catalog-v1.sqlite"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )?;
        let id = i64::try_from(id)?;
        let sessions = connection.query_row(
            "SELECT COUNT(*) FROM native_read_session_v1 WHERE id=?1",
            [id],
            |row| row.get(0),
        )?;
        let pins = connection.query_row(
            "SELECT COUNT(*) FROM native_pin_v1 WHERE session_id=?1",
            [id],
            |row| row.get(0),
        )?;
        Ok((sessions, pins))
    };
    assert_eq!(session_counts(dead_session_id)?, (1, 1));
    assert!(session_path(dead_session_id).is_file());
    // A separate orphan fixture exercises service-level cleanup; the killed
    // child left the catalog pin, not this scratch allocation.
    let scratch_orphan = root.path().join("native-scratch-v1/alloc-crashed-owner");
    std::fs::create_dir(&scratch_orphan)?;
    std::fs::set_permissions(&scratch_orphan, std::fs::Permissions::from_mode(0o700))?;
    std::fs::write(scratch_orphan.join("partial.sqlite"), b"orphan")?;
    // The live negative control is also born before the next physical seal.
    let live_catalog = SqliteCatalog::open(root.path(), Duration::from_millis(100))?;
    let live_session = live_catalog.open_read_session()?;
    let live_ticket = live_catalog.acquire_ticket(
        live_session.as_ref(),
        ViewSelection::Exact(genesis),
        Duration::from_secs(30),
        &WorkBudget::with_request(work_limits(), budget()),
    )?;
    let live_session_id = live_session.binding().id;
    assert_ne!(dead_session_id, live_session_id);
    drop(live_catalog);
    assert_eq!(session_counts(live_session_id)?, (1, 1));
    let service = bind_service(root.path(), &socket, work_limits(), descriptors())
        .map_err(|error| format!("first post-kill native bind: {error:#}"))?;
    assert!(!scratch_orphan.exists());
    let maintenance = service.maintenance_handle();
    let shutdown = service.shutdown_handle();
    let worker = std::thread::spawn(move || service.run(Duration::from_millis(5)));
    let _recovery = maintenance.recover_read_sessions(
        None,
        256,
        &mut WorkBudget::with_request(work_limits(), budget()),
    )?;
    let client = NativeUdsClient::new(socket);
    let latest = client.read_view(
        ViewSelection::Latest {
            workspace: workspace(),
            epoch: 1,
        },
        &budget(),
    )?;
    assert_eq!(latest.identity, genesis);
    shutdown.trigger();
    worker
        .join()
        .map_err(|_panic| "native reopen server thread panicked")??;
    let _post_shutdown_recovery = maintenance.recover_read_sessions(
        None,
        256,
        &mut WorkBudget::with_request(work_limits(), budget()),
    )?;
    assert!(
        bind_service(
            root.path(),
            &root.path().join("native.sock"),
            work_limits(),
            descriptors()
        )
        .is_err()
    );
    drop(maintenance);
    // Boot and explicit recovery may split the work. Durable rows and real
    // lock paths establish both outcomes after all admitted owners retire.
    assert_eq!(session_counts(dead_session_id)?, (0, 0));
    assert!(!session_path(dead_session_id).exists());
    assert_eq!(session_counts(live_session_id)?, (1, 1));
    assert!(session_path(live_session_id).is_file());
    let live_catalog = SqliteCatalog::open(root.path(), Duration::from_millis(100))?;
    live_catalog.validate_ticket(live_session.as_ref(), live_ticket.lease)?;
    live_catalog.release_ticket(live_session.as_ref(), live_ticket.lease)?;
    drop(live_session);
    drop(live_catalog);
    let rebound = bind_service(
        root.path(),
        &root.path().join("native.sock"),
        work_limits(),
        descriptors(),
    )
    .map_err(|error| format!("final native rebound after maintenance retirement: {error:#}"))?;
    drop(rebound);
    assert_eq!(session_counts(live_session_id)?, (0, 0));
    assert!(!session_path(live_session_id).exists());
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
        let error = match bind_service(root.path(), &socket, work_limits(), descriptors()) {
            Ok(service) => {
                drop(service);
                return Err("nested socket unexpectedly accepted".into());
            }
            Err(error) => error,
        };
        assert!(error.to_string().contains("nested directory"));
        assert!(!root.path().join(".native-index-v18").exists());
        assert!(!root.path().join("catalog").exists());
        assert!(!root.path().join("native-objects").exists());
        assert!(!root.path().join("run").exists());
    }
    let traversal = root.path().join("missing/../native.sock");
    let error = match bind_service(root.path(), &traversal, work_limits(), descriptors()) {
        Ok(service) => {
            drop(service);
            return Err("unresolved parent traversal unexpectedly accepted".into());
        }
        Err(error) => error,
    };
    assert!(error.to_string().contains("unresolved parent traversal"));
    assert!(!root.path().join(".native-index-v18").exists());
    assert!(!root.path().join("missing").exists());
    Ok(())
}

struct NativeFixedTextProvider {
    calls: Option<Arc<AtomicUsize>>,
}

impl TextEmbeddingProvider for NativeFixedTextProvider {
    fn embed_batch(
        &self,
        texts: &[&str],
    ) -> Result<quanta_index_core::TextEmbeddingBatch, CoreError> {
        if let Some(calls) = &self.calls {
            let _previous = calls.fetch_add(1, Ordering::SeqCst);
        }
        texts
            .iter()
            .map(|text| match *text {
                "alpha" | "alph" | "a" | "query" => Ok(vec![1.0, 0.0]),
                "beta" => Ok(vec![0.0, 1.0]),
                other => Err(CoreError::InvalidContract(format!(
                    "unexpected fixed text: {other}"
                ))),
            })
            .collect()
    }
    fn model_id(&self) -> &'static str {
        "native-uds-fixed-model"
    }
    fn model_revision(&self) -> &'static str {
        "fixed-revision-1"
    }
    fn dimension(&self) -> usize {
        2
    }
    fn normalization(&self) -> EmbeddingNormalization {
        EmbeddingNormalization::L2Unit
    }
}

fn semantic_request(text: &str) -> SemanticQueryRequest {
    SemanticQueryRequest {
        query_text: text.into(),
        constraints: QueryConstraintSetV1::unconstrained(),
        generation: None,
        generation_selector: None,
        lexical_scope: None,
        top_k: 2,
    }
}

fn bind_native_semantic_service(
    root: &std::path::Path,
    socket: &std::path::Path,
    max_owners: u32,
    with_pump: bool,
    chunk_policy: Option<quanta_index_semantic::native::Utf8ChunkPolicy>,
) -> TestResult<(NativeIndexService, InstanceSpec)> {
    let input = chunk_policy.map_or(
        SemanticFixtureInput::WholeFile,
        SemanticFixtureInput::FileLocal,
    );
    bind_native_semantic_service_with_input(root, socket, max_owners, with_pump, &input)
}

enum SemanticFixtureInput {
    WholeFile,
    FileLocal(quanta_index_semantic::native::Utf8ChunkPolicy),
    Certified {
        projection: ProjectionId,
        producer: ProducerId,
        schema: SchemaId,
        provider_calls: Arc<AtomicUsize>,
    },
}

fn bind_native_semantic_service_with_input(
    root: &std::path::Path,
    socket: &std::path::Path,
    max_owners: u32,
    with_pump: bool,
    input: &SemanticFixtureInput,
) -> TestResult<(NativeIndexService, InstanceSpec)> {
    let provider = NativeFixedTextProvider {
        calls: match input {
            SemanticFixtureInput::Certified { provider_calls, .. } => {
                Some(Arc::clone(provider_calls))
            }
            SemanticFixtureInput::WholeFile | SemanticFixtureInput::FileLocal(_) => None,
        },
    };
    let bridge = Arc::new(match input {
        SemanticFixtureInput::WholeFile => TextEmbeddingBridge::new(provider)?,
        SemanticFixtureInput::FileLocal(policy) => {
            TextEmbeddingBridge::with_utf8_chunks(provider, *policy)?
        }
        SemanticFixtureInput::Certified { .. } => {
            TextEmbeddingBridge::with_certified_chunks(provider)?
        }
    });
    let recipe = match input {
        SemanticFixtureInput::WholeFile => bridge.recipe(1024)?,
        SemanticFixtureInput::FileLocal(_) => bridge.chunk_recipe(1024, 4)?,
        SemanticFixtureInput::Certified {
            projection,
            producer,
            schema,
            ..
        } => bridge.certified_chunk_recipe(*projection, *producer, *schema, 1024, 4)?,
    };
    let mut spec = InstanceSpec {
        policy: recipe.fingerprint(),
        ..async_owner_spec()
    };
    if let SemanticFixtureInput::Certified {
        projection,
        producer,
        schema,
        ..
    } = input
    {
        spec.input_domain = InputDomain::Projection {
            projection: *projection,
            producer: *producer,
        };
        spec.schema = *schema;
    }
    let descriptor = AdapterDescriptor {
        kind: spec.kind,
        schema: spec.schema,
        format: spec.format,
    };
    let text_recipe = TextRecipe::new(Digest([31; 32]));
    let text_spec = native_text_spec();
    let text_descriptor = AdapterDescriptor {
        kind: text_spec.kind,
        schema: text_spec.schema,
        format: text_spec.format,
    };
    let mut builders = AdapterRegistry::new();
    builders.register_owner(
        descriptor,
        NativeSemantic::new(Arc::clone(&bridge), recipe.clone())?,
    )?;
    builders.register(
        text_descriptor,
        NativeTextIndex::new(text_recipe),
        |delta| Ok(delta.clone()),
    )?;
    let mut resources = test_resources();
    resources.execution.interactive.provider_units = 1;
    resources.execution.capacity.provider_units = 2;
    resources.execution.capacity.cpu_units = 5;
    resources.execution.capacity.io_bytes = 320 * 1024 * 1024;
    resources.execution.capacity.memory_bytes = (5 + 1) * 64 * 1024 * 1024;
    let mut semantic = QueryAdapterRegistry::<
        quanta_index_semantic::native::SemanticReadRequest,
        SearchResult,
    >::new();
    let need = resources.execution.interactive;
    semantic.register(
        descriptor,
        need,
        NativeSemanticReader::new(
            bridge,
            recipe,
            NativeSemanticReadConfig {
                page_size: 1,
                max_owners,
                max_query_bytes: 1024,
                need,
            },
        )?,
    )?;
    let mut text = QueryAdapterRegistry::<PlannedTextRead, TextReadResult>::new();
    let text_need = ResourceNeed {
        provider_units: 0,
        ..need
    };
    text.register(
        text_descriptor,
        text_need,
        NativeTextReader::new(text_recipe, Bm25Policy { k1: 1.2, b: 0.75 }, text_need)?,
    )?;
    let pump = with_pump.then(|| NativePumpConfig {
        scopes: vec![NativePumpScope {
            workspace: workspace(),
            epoch: 1,
        }],
        batch_limit: 16,
        max_in_flight: 1,
        poll_interval: Duration::from_millis(20),
        new_assignment: quanta_index_core::index::OwnedWorkPolicy {
            dispatch_limits: work_limits(),
            max_dispatches: 2,
            total_timeout_ms: 10_000,
            lease_ms: 2_000,
            retry_backoff_ms: 100,
        },
        cadence: Arc::new(FixedPumpPolicy),
    });
    let service = NativeIndexService::bind_with_capabilities(
        root,
        socket,
        work_limits(),
        NativeServiceCapabilities {
            builders,
            literal: QueryAdapterRegistry::new(),
            path: QueryAdapterRegistry::new(),
            text,
            semantic,
            pump,
            provider_attempt_pool: None,
        },
        resources,
    )?;
    Ok((service, spec))
}

const CERTIFIED_PROJECTION: ProjectionId = ProjectionId([20; 16]);
const CERTIFIED_PRODUCER: ProducerId = ProducerId([15; 16]);
const CERTIFIED_SCHEMA: SchemaId = SchemaId([22; 16]);
const CERTIFIED_ALPHA_DIGEST: Digest = Digest([
    0x8e, 0xd3, 0xf6, 0xad, 0x68, 0x5b, 0x95, 0x9e, 0xad, 0x70, 0x22, 0x51, 0x8e, 0x1a, 0xf7, 0x6c,
    0xd8, 0x16, 0xf8, 0xe8, 0xec, 0x7c, 0xcd, 0xda, 0x1e, 0xd4, 0x01, 0x8e, 0x8f, 0x22, 0x23, 0xf8,
]);

macro_rules! certified_check {
    ($condition:expr $(,)?) => {
        if !$condition {
            return Err(format!("assertion failed: {}", stringify!($condition)).into());
        }
    };
}

macro_rules! certified_check_eq {
    ($left:expr, $right:expr $(,)?) => {
        if &$left != &$right {
            return Err(format!(
                "assertion failed: {} == {}",
                stringify!($left),
                stringify!($right)
            )
            .into());
        }
    };
}

fn certified_owner() -> OwnerKey {
    OwnerKey {
        producer: CERTIFIED_PRODUCER,
        stable_id: StableOwnerId([1; 16]),
    }
}

struct CertifiedProjectionUpdate<'a> {
    view: quanta_index_contract::index::ViewRef,
    source_record: ObjectRef,
    prior: Option<ProjectionBinding>,
    sequence: u64,
    expected: Option<ObjectRef>,
    members: &'a [NativeSemanticManifestMember<'a>],
    forge_owner: bool,
}

fn stage_certified_projection(
    client: &NativeUdsClient,
    update: &CertifiedProjectionUpdate<'_>,
) -> TestResult<(quanta_index_contract::index::ViewRef, ObjectRef, ObjectRef)> {
    let key = OperationKey {
        producer: CERTIFIED_PRODUCER,
        stream: StreamId([7; 16]),
        sequence: update.sequence,
        ..operation(1)
    };
    let ingress = client.begin_input(key, &budget())?;
    let stamp = Digest([u8::try_from(update.sequence)?; 32]);
    let mut header = NativeSemanticManifestHeader {
        owner: certified_owner(),
        projection: CERTIFIED_PROJECTION,
        producer: CERTIFIED_PRODUCER,
        schema: CERTIFIED_SCHEMA,
        source: Some(update.source_record),
        dependency_stamp: stamp,
    };
    if update.forge_owner {
        header.owner.stable_id = StableOwnerId([2; 16]);
    }
    let mut encode_budget = WorkBudget::with_request(work_limits(), budget());
    let encoded = encode_certified_manifest(header, update.members, &mut encode_budget)?;
    let output = client.stage_projection_output(ingress, encoded.as_ref(), &budget())?;
    let contribution = ProjectionOwnerContribution {
        projection: CERTIFIED_PROJECTION,
        producer: CERTIFIED_PRODUCER,
        owner: certified_owner(),
        source_owner: Some(owner(1)),
        source: Some(update.source_record),
        dependency_stamp: stamp,
        output: Some(output),
    };
    let contribution_ref = client.stage_projection_owner(ingress, contribution, &budget())?;
    let affected = client.stage_mutations(
        ingress,
        [OwnerMutation {
            owner: certified_owner(),
            expected: update.expected,
            next: Some(contribution_ref),
        }],
        &budget(),
    )?;
    let source = client
        .read_view(ViewSelection::Exact(update.view), &budget())?
        .source;
    let mut transition = ProjectionTransition {
        projection: CERTIFIED_PROJECTION,
        producer: CERTIFIED_PRODUCER,
        expected: update.prior,
        target_source: source,
        schema: CERTIFIED_SCHEMA,
        producer_policy: Digest([23; 32]),
        dependency_context: Digest([24; 32]),
        completeness: ProducerCompleteness::Partial {
            missing: MapRoot(None),
        },
        owners: affected,
        events: Vec::new(),
    };
    let certificate = ProjectionCertificate {
        projection: CERTIFIED_PROJECTION,
        producer: CERTIFIED_PRODUCER,
        prior: update.prior,
        target_source: source,
        schema: CERTIFIED_SCHEMA,
        producer_policy: transition.producer_policy,
        dependency_context: transition.dependency_context,
        affected,
    };
    let certificate_ref = client.stage_projection_certificate(ingress, certificate, &budget())?;
    transition.completeness = ProducerCompleteness::CompleteAffectedDomain {
        certificate: certificate_ref,
    };
    transition.events.push(SourceEvent {
        producer: key.producer,
        stream: key.stream,
        stream_epoch: 1,
        predecessor: update
            .sequence
            .checked_sub(1)
            .ok_or("projection sequence underflow")?,
        sequence: update.sequence,
        payload: transition.payload_digest()?,
    });
    let next = published(&client.submit(
        UpdateRequest {
            operation: key,
            expected_view: update.view,
            kind: UpdateKind::ProjectionAdvance(Box::new(transition)),
        },
        &budget(),
    )?)?;
    Ok((next, contribution_ref, output))
}

fn claim_certified_semantic(
    client: &NativeUdsClient,
    instance: IndexInstanceId,
) -> TestResult<WorkClaim> {
    let NativeIndexResponse::ReadyWork(ready) = client.request(
        NativeIndexRequest::ReadyWork {
            workspace: workspace(),
            epoch: 1,
            limit: 16,
        },
        &budget(),
    )?
    else {
        return Err("certified semantic ready reply differs".into());
    };
    let mut matching = ready
        .into_iter()
        .filter(|intent| intent.instance == instance);
    let intent = matching.next().ok_or("certified semantic work absent")?;
    if matching.next().is_some() {
        return Err("certified semantic work is not owner local".into());
    }
    let NativeIndexResponse::Claimed(claim) = client.request(
        NativeIndexRequest::ClaimWork {
            intent,
            lease_millis: 60_000,
        },
        &budget(),
    )?
    else {
        return Err("certified semantic claim reply differs".into());
    };
    Ok(claim)
}

fn materialize_certified_semantic(
    client: &NativeUdsClient,
    view: quanta_index_contract::index::ViewRef,
    instance: IndexInstanceId,
    sequence: u64,
) -> TestResult<quanta_index_contract::index::ViewRef> {
    let claim = claim_certified_semantic(client, instance)?;
    let key = OperationKey {
        producer: ProducerId([91; 16]),
        stream: StreamId([92; 16]),
        sequence,
        ..operation(1)
    };
    published(&client.submit(
        UpdateRequest {
            operation: key,
            expected_view: view,
            kind: UpdateKind::Materialize { claim },
        },
        &budget(),
    )?)
}

/// Fixed provider vectors cross the actual SDK/UDS and persisted owner work.
/// Complete coverage, scan caps and old snapshots have independent oracles.
#[test]
fn native_uds_semantic_query_fixed_cosine_coverage_reopen_and_composed_pump() -> TestResult {
    use quanta_index_contract::index::CoverageStatus;
    let root = private_root()?;
    let socket = root.path().join("native.sock");
    let (bound, spec) = bind_native_semantic_service(root.path(), &socket, 16, false, None)?;
    let service = NativeServiceGuard::start(bound);
    let client = NativeUdsClient::new(socket.clone());
    let NativeIndexResponse::Created(genesis) = client.request(
        NativeIndexRequest::CreateWorkspace {
            workspace: workspace(),
            epoch: 1,
        },
        &budget(),
    )?
    else {
        return Err("semantic genesis reply differs".into());
    };
    let ingress = client.begin_input(operation(1), &budget())?;
    let profile = client.stage_profile(
        ingress,
        IndexProfile {
            instances: vec![native_text_spec(), spec.clone()],
        },
        &budget(),
    )?;
    let configured = published(&client.submit(
        UpdateRequest {
            operation: operation(1),
            expected_view: genesis,
            kind: UpdateKind::Reconfigure { profile },
        },
        &budget(),
    )?)?;
    let ingress = client.begin_input(operation(2), &budget())?;
    let records = client.stage_sources(
        ingress,
        vec![
            SourceInput {
                path: "alpha.rs".into(),
                bytes: b"alpha".to_vec(),
                text: SourceTextAdmission::Utf8 {
                    policy: Digest([31; 32]),
                },
            },
            SourceInput {
                path: "beta.rs".into(),
                bytes: b"beta".to_vec(),
                text: SourceTextAdmission::Utf8 {
                    policy: Digest([31; 32]),
                },
            },
        ],
        &budget(),
    )?;
    let prior = client.read_view(ViewSelection::Exact(configured), &budget())?;
    let owners = client.stage_mutations(
        ingress,
        [1, 2]
            .into_iter()
            .zip(&records)
            .map(|(id, record)| OwnerMutation {
                owner: owner(id),
                expected: None,
                next: Some(*record),
            }),
        &budget(),
    )?;
    let mut transition = SourceTransition {
        expected_source: prior.source,
        owners,
        events: Vec::new(),
    };
    transition.events.push(SourceEvent {
        producer: operation(2).producer,
        stream: operation(2).stream,
        stream_epoch: 1,
        predecessor: 0,
        sequence: 1,
        payload: transition.owner_digest()?,
    });
    let pending_view = published(&client.submit(
        UpdateRequest {
            operation: operation(2),
            expected_view: configured,
            kind: UpdateKind::SourceAdvance(transition),
        },
        &budget(),
    )?)?;
    let pending = client.search_semantic(
        ViewSelection::Exact(pending_view),
        spec.instance,
        semantic_request("query"),
        &budget(),
    )?;
    assert!(!pending.exhaustive);
    assert!(pending.hits.is_empty());
    assert!(!matches!(pending.coverage.status, CoverageStatus::Complete));
    let NativeIndexResponse::ReadyWork(work) = client.request(
        NativeIndexRequest::ReadyWork {
            workspace: workspace(),
            epoch: 1,
            limit: 16,
        },
        &budget(),
    )?
    else {
        return Err("semantic work reply differs".into());
    };
    let work: Vec<_> = work
        .into_iter()
        .filter(|intent| intent.instance == spec.instance)
        .collect();
    assert_eq!(work.len(), 2);
    let mut complete_view = pending_view;
    for (index, intent) in work.into_iter().enumerate() {
        let NativeIndexResponse::Claimed(claim) = client.request(
            NativeIndexRequest::ClaimWork {
                intent,
                lease_millis: 60_000,
            },
            &budget(),
        )?
        else {
            return Err("semantic claim reply differs".into());
        };
        let key = OperationKey {
            producer: ProducerId([91; 16]),
            stream: StreamId([92; 16]),
            sequence: u64::try_from(index)? + 1,
            ..operation(1)
        };
        complete_view = published(&client.submit(
            UpdateRequest {
                operation: key,
                expected_view: complete_view,
                kind: UpdateKind::Materialize { claim },
            },
            &budget(),
        )?)?;
    }
    let ranked = client.search_semantic(
        ViewSelection::Exact(complete_view),
        spec.instance,
        semantic_request("query"),
        &budget(),
    )?;
    assert_eq!(ranked.view, complete_view);
    assert_eq!(ranked.coverage.status, CoverageStatus::Complete);
    assert_eq!(ranked.coverage.covered, Some(ranked.coverage.desired));
    assert!(ranked.exhaustive);
    assert_eq!((ranked.scanned_owners, ranked.skipped_stale), (2, 0));
    assert_eq!(ranked.hits.len(), 2);
    for (hit, (expected_owner, expected_cosine)) in ranked
        .hits
        .iter()
        .zip([(owner(1), 1.0_f64), (owner(2), 0.0_f64)])
    {
        assert_eq!(hit.owner, expected_owner);
        assert_eq!(hit.vector_id, b"whole-file");
        assert_eq!(hit.score.get().to_bits(), expected_cosine.to_bits());
    }
    for text in ["", &"x".repeat(1025)] {
        assert!(matches!(
            client.request(NativeIndexRequest::SearchSemantic {
                selection: ViewSelection::Exact(complete_view), instance: spec.instance,
                query: semantic_request(text),
            }, &budget()),
            Err(NativeUdsError::Refused(error)) if error.code == NativeIndexErrorCode::InvalidInput
        ));
    }
    let mut scoped = semantic_request("query");
    scoped.lexical_scope = Some(native_text_request("alpha", 2));
    assert!(matches!(
        client.request(NativeIndexRequest::SearchSemantic {
            selection: ViewSelection::Exact(complete_view), instance: spec.instance, query: scoped,
        }, &budget()),
        Err(NativeUdsError::Refused(error)) if error.code == NativeIndexErrorCode::InvalidInput
    ));
    let old = client.search_semantic(
        ViewSelection::Exact(pending_view),
        spec.instance,
        semantic_request("query"),
        &budget(),
    )?;
    assert_eq!(old, pending);
    let text = client.search_text(
        ViewSelection::Exact(complete_view),
        native_text_spec().instance,
        native_text_request("alpha", 2),
        &budget(),
    )?;
    assert_eq!(text.hits.len(), 1);
    assert_eq!(text.hits.first().ok_or("text hit absent")?.owner, owner(1));
    service.stop()?;
    let (bound, _) = bind_native_semantic_service(root.path(), &socket, 1, true, None)?;
    let reopened = NativeServiceGuard::start(bound);
    let capped = client.search_semantic(
        ViewSelection::Exact(complete_view),
        spec.instance,
        semantic_request("query"),
        &budget(),
    )?;
    assert_eq!(capped.coverage, ranked.coverage);
    assert!(!capped.exhaustive);
    assert_eq!(capped.scanned_owners, 1);
    assert_eq!(capped.hits.len(), 1);
    assert_eq!(
        capped.hits.first().ok_or("capped hit absent")?.owner,
        owner(1)
    );
    let (ingress, record) = stage_source(&client, operation(3), "gamma.rs", b"alpha")?;
    let advanced = advance_one(&client, ingress, complete_view, owner(3), record, 2)?;
    let until = Instant::now()
        .checked_add(Duration::from_secs(10))
        .ok_or("semantic pump test deadline overflow")?;
    loop {
        let latest = client.search_semantic(
            ViewSelection::Latest {
                workspace: workspace(),
                epoch: 1,
            },
            spec.instance,
            semantic_request("query"),
            &budget(),
        )?;
        if latest.coverage.status == CoverageStatus::Complete && latest.view != advanced {
            assert!(!latest.exhaustive);
            break;
        }
        if Instant::now() >= until {
            return Err("configured semantic pump did not complete new owner".into());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let retained = client.search_semantic(
        ViewSelection::Exact(complete_view),
        spec.instance,
        semantic_request("query"),
        &budget(),
    )?;
    assert_eq!(retained, capped);
    reopened.stop()
}

#[test]
fn native_uds_semantic_chunk_bindings_survive_materialization_and_reopen() -> TestResult {
    use quanta_index_contract::index::CoverageStatus;
    use quanta_index_contract::native_protocol::NativeSemanticChunkSpan;

    let root = private_root()?;
    let socket = root.path().join("native.sock");
    let policy = quanta_index_semantic::native::Utf8ChunkPolicy { max_utf8_bytes: 4 };
    let (bound, spec) =
        bind_native_semantic_service(root.path(), &socket, 16, false, Some(policy))?;
    let service = NativeServiceGuard::start(bound);
    let client = NativeUdsClient::new(socket.clone());

    let NativeIndexResponse::Created(genesis) = client.request(
        NativeIndexRequest::CreateWorkspace {
            workspace: workspace(),
            epoch: 1,
        },
        &budget(),
    )?
    else {
        return Err("chunk semantic genesis reply differs".into());
    };
    let ingress = client.begin_input(operation(1), &budget())?;
    let profile = client.stage_profile(
        ingress,
        IndexProfile {
            instances: vec![spec.clone()],
        },
        &budget(),
    )?;
    let configured = published(&client.submit(
        UpdateRequest {
            operation: operation(1),
            expected_view: genesis,
            kind: UpdateKind::Reconfigure { profile },
        },
        &budget(),
    )?)?;
    let ingress = client.begin_input(operation(2), &budget())?;
    let records = client.stage_sources(
        ingress,
        vec![SourceInput {
            path: "alpha.rs".into(),
            bytes: b"alpha".to_vec(),
            text: SourceTextAdmission::Utf8 {
                policy: Digest([31; 32]),
            },
        }],
        &budget(),
    )?;
    let [source_record] = records.as_slice() else {
        return Err("chunk source stage did not return exactly one record".into());
    };
    let source_record = *source_record;
    let pending = advance_one(&client, ingress, configured, owner(1), source_record, 1)?;
    let NativeIndexResponse::ReadyWork(ready) = client.request(
        NativeIndexRequest::ReadyWork {
            workspace: workspace(),
            epoch: 1,
            limit: 16,
        },
        &budget(),
    )?
    else {
        return Err("chunk semantic ready reply differs".into());
    };
    let mut semantic = ready
        .into_iter()
        .filter(|intent| intent.instance == spec.instance);
    let intent = semantic.next().ok_or("chunk semantic work absent")?;
    if semantic.next().is_some() {
        return Err("chunk semantic work is not owner local".into());
    }
    let NativeIndexResponse::Claimed(claim) = client.request(
        NativeIndexRequest::ClaimWork {
            intent,
            lease_millis: 60_000,
        },
        &budget(),
    )?
    else {
        return Err("chunk semantic claim reply differs".into());
    };
    let materialize_key = OperationKey {
        producer: ProducerId([91; 16]),
        stream: StreamId([92; 16]),
        sequence: 1,
        ..operation(1)
    };
    let complete = published(&client.submit(
        UpdateRequest {
            operation: materialize_key,
            expected_view: pending,
            kind: UpdateKind::Materialize { claim },
        },
        &budget(),
    )?)?;
    let ranked = client.search_semantic(
        ViewSelection::Exact(complete),
        spec.instance,
        semantic_request("query"),
        &budget(),
    )?;
    assert_eq!(ranked.coverage.status, CoverageStatus::Complete);
    assert!(ranked.exhaustive);
    assert_eq!(ranked.hits.len(), 2);
    let mut selected_payload = None;
    let mut observed = Vec::new();
    for hit in &ranked.hits {
        assert_eq!(hit.owner, owner(1));
        let chunk = hit.chunk.ok_or("chunk metadata absent from SDK reply")?;
        assert_eq!(hit.vector_id.as_slice(), chunk.id.0.as_slice());
        assert_eq!(chunk.input.source, Some(source_record));
        assert_eq!(chunk.input.projection, None);
        assert_eq!(chunk.input.producer_context, Digest([0; 32]));
        assert_eq!(
            chunk.input.derivation_policy,
            ranked.coverage.desired.policy
        );
        if let Some(prior) = selected_payload {
            assert_eq!(chunk.payload, prior);
        } else {
            selected_payload = Some(chunk.payload);
        }
        let NativeSemanticChunkSpan::SourcePayload {
            start_byte,
            end_byte,
        } = chunk.span
        else {
            return Err("source chunk was reported as projection payload".into());
        };
        let mut input_digest = String::with_capacity(64);
        for byte in chunk.input_digest.0 {
            write!(&mut input_digest, "{byte:02x}")?;
        }
        observed.push((start_byte, end_byte, input_digest));
    }
    observed.sort_unstable_by_key(|(start, _, _)| *start);
    assert_eq!(
        observed,
        [
            (
                0,
                4,
                "2429a99e01b351de987b5081cb49e4d8f9af4d1a7269cc9c0be93690f1b686a3".to_owned(),
            ),
            (
                4,
                5,
                "ca978112ca1bbdcafac231b39a23dc4da786eff8147c4e72b9807785afee48bb".to_owned(),
            ),
        ]
    );

    service.stop()?;
    let (bound, reopened_spec) =
        bind_native_semantic_service(root.path(), &socket, 16, false, Some(policy))?;
    assert_eq!(reopened_spec.policy, spec.policy);
    let reopened = NativeServiceGuard::start(bound);
    let old_exact = client.search_semantic(
        ViewSelection::Exact(complete),
        spec.instance,
        semantic_request("query"),
        &budget(),
    )?;
    assert_eq!(old_exact, ranked);
    let current = client.search_semantic(
        ViewSelection::Latest {
            workspace: workspace(),
            epoch: 1,
        },
        spec.instance,
        semantic_request("query"),
        &budget(),
    )?;
    assert_eq!(current, ranked);
    reopened.stop()
}

#[test]
fn native_uds_certified_chunks_survive_projection_delta_reopen_and_refuse_forged_header()
-> TestResult {
    use quanta_index_contract::index::{CoverageStatus, Refusal};

    let root = private_root()?;
    let socket = root.path().join("native.sock");
    let calls = Arc::new(AtomicUsize::new(0));
    let certified = || SemanticFixtureInput::Certified {
        projection: CERTIFIED_PROJECTION,
        producer: CERTIFIED_PRODUCER,
        schema: CERTIFIED_SCHEMA,
        provider_calls: Arc::clone(&calls),
    };
    let (bound, spec) =
        bind_native_semantic_service_with_input(root.path(), &socket, 16, false, &certified())?;
    let service = NativeServiceGuard::start(bound);
    let client = NativeUdsClient::new(socket.clone());

    let NativeIndexResponse::Created(genesis) = client.request(
        NativeIndexRequest::CreateWorkspace {
            workspace: workspace(),
            epoch: 1,
        },
        &budget(),
    )?
    else {
        return Err("certified semantic genesis reply differs".into());
    };
    let ingress = client.begin_input(operation(1), &budget())?;
    let profile = client.stage_profile(
        ingress,
        IndexProfile {
            instances: vec![spec.clone()],
        },
        &budget(),
    )?;
    let configured = published(&client.submit(
        UpdateRequest {
            operation: operation(1),
            expected_view: genesis,
            kind: UpdateKind::Reconfigure { profile },
        },
        &budget(),
    )?)?;
    let (source_ingress, source_record) =
        stage_source(&client, operation(2), "symbols.rs", b"alpha")?;
    let sourced = advance_one(
        &client,
        source_ingress,
        configured,
        owner(1),
        source_record,
        1,
    )?;

    let original_members = [
        NativeSemanticManifestMember {
            id: Digest([11; 32]),
            text: "alpha",
        },
        NativeSemanticManifestMember {
            id: Digest([12; 32]),
            text: "beta",
        },
    ];
    let (first_pending, first_contribution, first_output) = stage_certified_projection(
        &client,
        &CertifiedProjectionUpdate {
            view: sourced,
            source_record,
            prior: None,
            sequence: 1,
            expected: None,
            members: &original_members,
            forge_owner: false,
        },
    )?;
    let pending = client.search_semantic(
        ViewSelection::Exact(first_pending),
        spec.instance,
        semantic_request("query"),
        &budget(),
    )?;
    certified_check!(pending.hits.is_empty());
    let first = materialize_certified_semantic(&client, first_pending, spec.instance, 1)?;
    let ranked = client.search_semantic(
        ViewSelection::Exact(first),
        spec.instance,
        semantic_request("query"),
        &budget(),
    )?;
    certified_check_eq!(ranked.coverage.status, CoverageStatus::Complete);
    certified_check_eq!(ranked.hits.len(), 2);
    let alpha = ranked
        .hits
        .iter()
        .find(|hit| hit.vector_id.as_slice() == [11_u8; 32].as_slice())
        .ok_or("certified alpha hit absent")?;
    let chunk = alpha.chunk.ok_or("certified alpha binding absent")?;
    certified_check_eq!(alpha.owner, certified_owner());
    certified_check_eq!(alpha.score.get().to_bits(), 1.0_f64.to_bits());
    certified_check_eq!(chunk.id, Digest([11; 32]));
    certified_check_eq!(chunk.input_digest, CERTIFIED_ALPHA_DIGEST);
    certified_check_eq!(chunk.payload, first_output);
    certified_check_eq!(chunk.input.projection, Some(first_contribution));
    certified_check_eq!(chunk.input.source, Some(source_record));
    certified_check_eq!(
        chunk.input.derivation_policy,
        ranked.coverage.desired.policy
    );
    certified_check_eq!(
        chunk.span,
        NativeSemanticChunkSpan::RenderedMember { ordinal: 0 }
    );
    chunk.validate_binding(&alpha.vector_id, ranked.coverage.desired.policy)?;

    let first_lookup = client.read_projection_binding(
        ViewSelection::Exact(first),
        CERTIFIED_PROJECTION,
        &budget(),
    )?;
    certified_check_eq!(first_lookup.view, first);
    let first_binding = first_lookup
        .binding
        .ok_or("certified projection binding absent")?;
    certified_check_eq!(first_binding.schema, CERTIFIED_SCHEMA);
    certified_check_eq!(first_binding.producer, CERTIFIED_PRODUCER);
    let shifted_members = [
        NativeSemanticManifestMember {
            id: Digest([13; 32]),
            text: "alph",
        },
        NativeSemanticManifestMember {
            id: Digest([11; 32]),
            text: "alpha",
        },
        NativeSemanticManifestMember {
            id: Digest([12; 32]),
            text: "beta",
        },
    ];
    let (second_pending, second_contribution, second_output) = stage_certified_projection(
        &client,
        &CertifiedProjectionUpdate {
            view: first,
            source_record,
            prior: Some(first_binding),
            sequence: 2,
            expected: Some(first_contribution),
            members: &shifted_members,
            forge_owner: false,
        },
    )?;
    let stale = client.search_semantic(
        ViewSelection::Exact(second_pending),
        spec.instance,
        semantic_request("query"),
        &budget(),
    )?;
    certified_check!(stale.hits.is_empty());
    let second = materialize_certified_semantic(&client, second_pending, spec.instance, 2)?;
    let mut query = semantic_request("query");
    query.top_k = 4;
    let updated = client.search_semantic(
        ViewSelection::Exact(second),
        spec.instance,
        query.clone(),
        &budget(),
    )?;
    certified_check_eq!(updated.coverage.status, CoverageStatus::Complete);
    certified_check_eq!(updated.hits.len(), 3);
    let moved = updated
        .hits
        .iter()
        .find(|hit| hit.vector_id.as_slice() == [11_u8; 32].as_slice())
        .ok_or("moved certified alpha hit absent")?;
    let moved_chunk = moved.chunk.ok_or("moved certified alpha binding absent")?;
    certified_check_eq!(moved_chunk.id, chunk.id);
    certified_check_eq!(moved_chunk.input_digest, CERTIFIED_ALPHA_DIGEST);
    certified_check_eq!(moved_chunk.payload, second_output);
    certified_check_eq!(moved_chunk.input.projection, Some(second_contribution));
    certified_check_eq!(
        moved_chunk.span,
        NativeSemanticChunkSpan::RenderedMember { ordinal: 1 }
    );

    let second_lookup = client.read_projection_binding(
        ViewSelection::Exact(second),
        CERTIFIED_PROJECTION,
        &budget(),
    )?;
    certified_check_eq!(second_lookup.view, second);
    let second_binding = second_lookup
        .binding
        .ok_or("updated certified projection binding absent")?;
    service.stop()?;
    let (bound, reopened_spec) =
        bind_native_semantic_service_with_input(root.path(), &socket, 16, false, &certified())?;
    certified_check_eq!(reopened_spec.policy, spec.policy);
    let reopened = NativeServiceGuard::start(bound);
    let exact_first = client.search_semantic(
        ViewSelection::Exact(first),
        spec.instance,
        semantic_request("query"),
        &budget(),
    )?;
    certified_check_eq!(exact_first, ranked);
    let exact_second = client.search_semantic(
        ViewSelection::Exact(second),
        spec.instance,
        query.clone(),
        &budget(),
    )?;
    certified_check_eq!(exact_second, updated);
    let latest = client.search_semantic(
        ViewSelection::Latest {
            workspace: workspace(),
            epoch: 1,
        },
        spec.instance,
        query,
        &budget(),
    )?;
    certified_check_eq!(latest, updated);

    let forged_members = [NativeSemanticManifestMember {
        id: Digest([11; 32]),
        text: "alpha",
    }];
    let (bad_pending, _, _) = stage_certified_projection(
        &client,
        &CertifiedProjectionUpdate {
            view: second,
            source_record,
            prior: Some(second_binding),
            sequence: 3,
            expected: Some(second_contribution),
            members: &forged_members,
            forge_owner: true,
        },
    )?;
    let before_bad = calls.load(Ordering::SeqCst);
    let claim = claim_certified_semantic(&client, spec.instance)?;
    let bad_key = OperationKey {
        producer: ProducerId([91; 16]),
        stream: StreamId([92; 16]),
        sequence: 3,
        ..operation(1)
    };
    let refused = client.submit(
        UpdateRequest {
            operation: bad_key,
            expected_view: bad_pending,
            kind: UpdateKind::Materialize { claim },
        },
        &budget(),
    );
    let invalid_input = match refused {
        Ok(SubmitOutcome::Terminal(OperationStatus::Refused(Refusal::InvalidInput))) => true,
        Err(NativeUdsError::Refused(error)) => error.code == NativeIndexErrorCode::InvalidInput,
        _ => false,
    };
    certified_check!(invalid_input);
    certified_check_eq!(calls.load(Ordering::SeqCst), before_bad);
    let bad_latest = client.search_semantic(
        ViewSelection::Exact(bad_pending),
        spec.instance,
        semantic_request("query"),
        &budget(),
    )?;
    certified_check!(bad_latest.hits.is_empty());
    reopened.stop()
}

#[test]
fn native_owned_pump_automatically_reclaims_discarded_inputs_without_manual_steps() -> TestResult {
    let root = private_root()?;
    let socket = root.path().join("native.sock");
    let mut resources = test_resources();
    // Four foreground classes plus one owned pump and one reclaim control slot.
    resources.execution.capacity.cpu_units = 6;
    resources.execution.capacity.io_bytes = 384 * 1024 * 1024;
    resources.execution.capacity.memory_bytes = (6 + 1) * 64 * 1024 * 1024;
    let pump = NativePumpConfig {
        scopes: vec![NativePumpScope {
            workspace: workspace(),
            epoch: 1,
        }],
        batch_limit: 1,
        max_in_flight: 1,
        poll_interval: Duration::from_millis(20),
        new_assignment: quanta_index_core::index::OwnedWorkPolicy {
            dispatch_limits: work_limits(),
            max_dispatches: 2,
            total_timeout_ms: 10_000,
            lease_ms: 2_000,
            retry_backoff_ms: 100,
        },
        cadence: Arc::new(FixedPumpPolicy),
    };
    let service = NativeIndexService::bind_with_capabilities_and_maintenance(
        root.path(),
        &socket,
        work_limits(),
        reclaim_capabilities(Some(pump))?,
        resources,
        Some(test_reclaim_config()),
    )?;
    let running = NativeServiceGuard::start(service);
    let client = NativeUdsClient::new(socket);
    let created = client.request(
        NativeIndexRequest::CreateWorkspace {
            workspace: workspace(),
            epoch: 1,
        },
        &budget(),
    )?;
    assert!(matches!(created, NativeIndexResponse::Created(_)));
    let discarded_body = b"automatic reclaim discarded source";
    let (discarded_ingress, discarded_record) =
        stage_source(&client, operation(860), "discarded.rs", discarded_body)?;
    let retained_body = b"automatic reclaim retained source";
    let (_retained_ingress, retained_record) =
        stage_source(&client, operation(861), "retained.rs", retained_body)?;
    let identity_root = private_root()?;
    let identity = FileObjectStore::open(identity_root.path().join("identity-objects"))?;
    let discarded_content = identity
        .identify(
            discarded_body,
            &[],
            &mut WorkBudget::with_request(work_limits(), budget()),
        )?
        .reference;
    let retained_content = identity
        .identify(
            retained_body,
            &[],
            &mut WorkBudget::with_request(work_limits(), budget()),
        )?
        .reference;
    let discarded_record_file = native_object_file(root.path(), discarded_record)?;
    let discarded_content_file = native_object_file(root.path(), discarded_content)?;
    let retained_record_file = native_object_file(root.path(), retained_record)?;
    let retained_content_file = native_object_file(root.path(), retained_content)?;
    assert!(discarded_record_file.is_file());
    assert!(discarded_content_file.is_file());
    assert!(retained_record_file.is_file());
    assert!(retained_content_file.is_file());
    client.discard_input(discarded_ingress, &budget())?;

    let deadline = Instant::now()
        .checked_add(Duration::from_secs(5))
        .ok_or("deadline overflow")?;
    while (discarded_record_file.exists() || discarded_content_file.exists())
        && Instant::now() < deadline
    {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        !discarded_record_file.exists(),
        "automatic driver retained discarded record"
    );
    assert!(
        !discarded_content_file.exists(),
        "automatic driver retained discarded content"
    );
    assert!(
        retained_record_file.is_file(),
        "automatic driver deleted retained record"
    );
    assert!(
        retained_content_file.is_file(),
        "automatic driver deleted retained content"
    );
    running.stop()
}

#[test]
fn native_automatic_reclaim_without_owned_pump_deletes_only_discarded_source() -> TestResult {
    let root = private_root()?;
    let socket = root.path().join("native.sock");
    let mut resources = test_resources();
    // The four protected foreground classes retain their full floors.
    resources.execution.capacity.cpu_units = 5;
    resources.execution.capacity.io_bytes = 320 * 1024 * 1024;
    resources.execution.capacity.memory_bytes = (5 + 1) * 64 * 1024 * 1024;
    let service = NativeIndexService::bind_with_capabilities_and_maintenance(
        root.path(),
        &socket,
        work_limits(),
        reclaim_capabilities(None)?,
        resources,
        Some(test_reclaim_config()),
    )?;
    let running = NativeServiceGuard::start(service);
    let client = NativeUdsClient::new(socket);
    let created = client.request(
        NativeIndexRequest::CreateWorkspace {
            workspace: workspace(),
            epoch: 1,
        },
        &budget(),
    )?;
    assert!(matches!(created, NativeIndexResponse::Created(_)));
    let removed_body = b"gc-only discarded source";
    let (removed_ingress, removed_record) =
        stage_source(&client, operation(870), "removed.rs", removed_body)?;
    let kept_body = b"gc-only retained source";
    let (_kept_ingress, kept_record) = stage_source(&client, operation(871), "kept.rs", kept_body)?;
    let identity_root = private_root()?;
    let identity = FileObjectStore::open(identity_root.path().join("identity-objects"))?;
    let removed_content = identity
        .identify(
            removed_body,
            &[],
            &mut WorkBudget::with_request(work_limits(), budget()),
        )?
        .reference;
    let kept_content = identity
        .identify(
            kept_body,
            &[],
            &mut WorkBudget::with_request(work_limits(), budget()),
        )?
        .reference;
    let removed_record_file = native_object_file(root.path(), removed_record)?;
    let removed_content_file = native_object_file(root.path(), removed_content)?;
    let kept_record_file = native_object_file(root.path(), kept_record)?;
    let kept_content_file = native_object_file(root.path(), kept_content)?;
    assert!(removed_record_file.is_file());
    assert!(removed_content_file.is_file());
    assert!(kept_record_file.is_file());
    assert!(kept_content_file.is_file());
    client.discard_input(removed_ingress, &budget())?;
    let deadline = Instant::now()
        .checked_add(Duration::from_secs(5))
        .ok_or("deadline overflow")?;
    while (removed_record_file.exists() || removed_content_file.exists())
        && Instant::now() < deadline
    {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        !removed_record_file.exists(),
        "GC-only driver retained discarded record"
    );
    assert!(
        !removed_content_file.exists(),
        "GC-only driver retained discarded body"
    );
    assert!(
        kept_record_file.is_file(),
        "GC-only driver deleted retained record"
    );
    assert!(
        kept_content_file.is_file(),
        "GC-only driver deleted retained body"
    );
    running.stop()
}

#[test]
fn native_automatic_reclaim_invalid_config_and_missing_floor_refuse_before_state() -> TestResult {
    for case in ["zero_interval", "low_work", "missing_floor"] {
        let root = private_root()?;
        let socket = root.path().join("native.sock");
        let mut config = test_reclaim_config();
        let mut resources = test_resources();
        resources.execution.capacity.cpu_units = 5;
        resources.execution.capacity.io_bytes = 320 * 1024 * 1024;
        resources.execution.capacity.memory_bytes = (5 + 1) * 64 * 1024 * 1024;
        match case {
            "zero_interval" => config.interval = Duration::ZERO,
            "low_work" => config.work_limits.bytes = 1,
            "missing_floor" => resources = test_resources(),
            _ => return Err("unknown automatic reclaim refusal case".into()),
        }
        let result = NativeIndexService::bind_with_capabilities_and_maintenance(
            root.path(),
            &socket,
            work_limits(),
            reclaim_capabilities(None)?,
            resources,
            Some(config),
        );
        assert!(result.is_err(), "{case} accepted");
        assert!(
            !root.path().join(".native-index-v18").exists(),
            "{case} wrote format"
        );
        assert!(
            !root.path().join("native-objects").exists(),
            "{case} wrote objects"
        );
        assert!(!socket.exists(), "{case} bound socket");
    }
    Ok(())
}

#[test]
fn native_automatic_reclaim_resumes_prepared_partial_child_drain_after_service_reopen() -> TestResult
{
    let root = private_root()?;
    let socket = root.path().join("native.sock");
    let seed_service = bind_service(root.path(), &socket, work_limits(), descriptors())?;
    let seed_running = NativeServiceGuard::start(seed_service);
    let client = NativeUdsClient::new(socket.clone());
    let created = client.request(
        NativeIndexRequest::CreateWorkspace {
            workspace: workspace(),
            epoch: 1,
        },
        &budget(),
    )?;
    assert!(matches!(created, NativeIndexResponse::Created(_)));
    let children_ingress = client.begin_input(operation(890), &budget())?;
    let parent_ingress = client.begin_input(operation(891), &budget())?;
    seed_running.stop()?;

    // Two durable ingress roots make the child objects live independently of
    // the parent. The parent needs three 32-edge drain pages after unlink.
    let catalog = Arc::new(SqliteCatalog::open(root.path(), Duration::from_secs(2))?);
    let scratch_root = private_root()?;
    let scratch = Arc::new(FileScratchSpace::open_standalone(
        scratch_root.path().join("scratch"),
        32 * 1024 * 1024,
    )?);
    let objects = FileObjectStore::open_managed_with_scratch(
        root.path().join("native-objects"),
        scratch,
        catalog.clone(),
    )?;
    let child_sink = FixtureIngressSink {
        catalog: catalog.as_ref(),
        ingress: children_ingress,
    };
    let parent_sink = FixtureIngressSink {
        catalog: catalog.as_ref(),
        ingress: parent_ingress,
    };
    let mut install_budget = WorkBudget::with_request(work_limits(), budget());
    let mut children = Vec::with_capacity(70);
    for ordinal in 0..70 {
        let payload = format!("retained-reclaim-child-{ordinal:03}");
        let installed =
            objects.install(payload.as_bytes(), &[], &child_sink, &mut install_budget)?;
        children.push(installed.reference);
    }
    let parent = objects.install(
        b"reclaim-parent-with-seventy-children",
        &children,
        &parent_sink,
        &mut install_budget,
    )?;
    let parent_file = native_object_file(root.path(), parent.reference)?;
    assert!(parent_file.is_file());
    for child in &children {
        assert!(native_object_file(root.path(), *child)?.is_file());
    }
    let mut child_wires = Vec::with_capacity(children.len());
    for (ordinal, child) in children.iter().enumerate() {
        let expected = format!("retained-reclaim-child-{ordinal:03}");
        let stored = objects.get(*child, &mut install_budget)?;
        assert_eq!(stored.registration.reference, *child);
        assert_eq!(stored.payload.as_slice(), expected.as_bytes());
        let path = native_object_file(root.path(), *child)?;
        child_wires.push((path.clone(), std::fs::read(path)?));
    }
    catalog.discard_inputs(
        parent_ingress,
        &mut WorkBudget::with_request(work_limits(), budget()),
    )?;
    let queued = catalog.reclaim_work_page(
        None,
        8,
        &mut WorkBudget::with_request(work_limits(), budget()),
    )?;
    assert_eq!(queued.items.len(), 1);
    assert_eq!(
        queued
            .items
            .first()
            .ok_or("missing parent reclaim work")?
            .object(),
        parent.reference
    );
    drop(objects);
    drop(catalog);

    let mut resources = test_resources();
    resources.execution.capacity.cpu_units = 5;
    resources.execution.capacity.io_bytes = 320 * 1024 * 1024;
    resources.execution.capacity.memory_bytes = (5 + 1) * 64 * 1024 * 1024;
    // Prepare exactly one durable 32-edge page through the public maintenance
    // port, with no automatic driver competing for a second page.
    let preparation = NativeIndexService::bind_with_capabilities_and_maintenance(
        root.path(),
        &socket,
        work_limits(),
        reclaim_capabilities(None)?,
        resources,
        None,
    )?;
    let first = preparation.maintenance_handle().reclaim_step(
        None,
        32,
        &mut WorkBudget::with_request(work_limits(), budget()),
    )?;
    assert_eq!(first.examined, Some(parent.reference));
    assert!(
        first
            .drain
            .is_some_and(|page| { page.released_children == 32 && page.has_more })
    );
    drop(preparation);
    let catalog = SqliteCatalog::open(root.path(), Duration::from_secs(2))?;
    let connection = rusqlite::Connection::open_with_flags(
        catalog.path(),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?;
    let prepared: (i64, i64, i64) = connection.query_row(
        "SELECT phase,direct_children,drained_children FROM native_gc_object_v1 WHERE object=?1",
        rusqlite::params![(parent.reference.0).0.to_vec()],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    assert_eq!(prepared, (3, 70, 32));
    assert!(
        native_object_absent_nofollow(&parent_file)?,
        "partial drain preceded physical parent unlink"
    );
    for (path, wire) in &child_wires {
        assert_eq!(std::fs::read(path)?.as_slice(), wire.as_slice());
    }
    drop(connection);

    let reopened =
        NativeServiceGuard::start(NativeIndexService::bind_with_capabilities_and_maintenance(
            root.path(),
            &socket,
            work_limits(),
            reclaim_capabilities(None)?,
            resources,
            Some(test_reclaim_config()),
        )?);
    let connection = rusqlite::Connection::open_with_flags(
        catalog.path(),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?;
    let deadline = Instant::now()
        .checked_add(Duration::from_secs(8))
        .ok_or("deadline overflow")?;
    loop {
        let lifecycle = connection.query_row(
            "SELECT phase,drained_children FROM native_gc_object_v1 WHERE object=?1",
            rusqlite::params![(parent.reference.0).0.to_vec()],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        );
        match lifecycle {
            Err(rusqlite::Error::QueryReturnedNoRows) => break,
            Ok((_phase, _drained)) => {}
            Err(error) => return Err(error.into()),
        }
        assert!(
            Instant::now() < deadline,
            "reopen did not finish parent's persisted drain"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    let parent_metadata: i64 = connection.query_row(
        "SELECT count(*) FROM native_object_v1 WHERE object=?1",
        rusqlite::params![(parent.reference.0).0.to_vec()],
        |row| row.get(0),
    )?;
    assert_eq!(parent_metadata, 0);
    assert!(native_object_absent_nofollow(&parent_file)?);
    for child in &children {
        assert!(native_object_file(root.path(), *child)?.is_file());
    }
    for (path, wire) in &child_wires {
        assert_eq!(std::fs::read(path)?.as_slice(), wire.as_slice());
    }
    drop(connection);
    drop(catalog);

    // The held ingress remains usable after reopen. Releasing it makes all
    // 70 now-unshared children eligible for automatic physical reclamation.
    NativeUdsClient::new(socket).discard_input(children_ingress, &budget())?;
    let deadline = Instant::now()
        .checked_add(Duration::from_secs(10))
        .ok_or("deadline overflow")?;
    while !native_objects_absent_nofollow(root.path(), &children)? && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(native_objects_absent_nofollow(root.path(), &children)?);
    reopened.stop()
}

#[test]
fn native_response_frame_pressure_releases_data_without_dispatch_slots() -> TestResult {
    use std::io::Write;
    use std::net::Shutdown;
    use std::os::unix::net::UnixStream;
    let root = private_root()?;
    let socket = root.path().join("native.sock");
    let service = NativeIndexService::bind(
        root.path(),
        &socket,
        work_limits(),
        descriptors(),
        test_resources(),
    )?;
    let observation = service.resource_observation();
    let running = NativeServiceGuard::start(service);
    let client = NativeUdsClient::new(socket.clone());
    let response = client.request(
        NativeIndexRequest::CreateWorkspace {
            workspace: workspace(),
            epoch: 1,
        },
        &budget(),
    )?;
    let NativeIndexResponse::Created(genesis) = response else {
        return Err("pressure fixture genesis missing".into());
    };
    let stream = UnixStream::connect(socket)?;
    rustix::net::sockopt::set_socket_recv_buffer_size(&stream, 4096)?;
    let mut sender = stream.try_clone()?;
    sender.set_write_timeout(Some(Duration::from_secs(2)))?;
    let writer = std::thread::spawn(move || -> std::io::Result<()> {
        for request_id in 1..=4096 {
            let envelope = quanta_index_contract::native_protocol::NativeIndexRequestEnvelope {
                request_id,
                payload: NativeIndexRequest::ReadView(ViewSelection::Exact(genesis)),
            };
            let frame =
                quanta_index_ipc::encode_request(&envelope).map_err(std::io::Error::other)?;
            sender.write_all(&frame)?;
        }
        Ok(())
    });
    let deadline = Instant::now()
        .checked_add(Duration::from_secs(10))
        .ok_or("pressure deadline overflow")?;
    let mut held = None;
    while Instant::now() < deadline {
        let snapshot = observation.observe()?;
        if snapshot.live_memory_bytes > 0
            && snapshot.live_memory_bytes == snapshot.retained_memory_bytes
            && snapshot.reserved == ResourceNeed::default()
            && snapshot.pending_escape_bytes == 0
        {
            std::thread::sleep(Duration::from_millis(75));
            let next = observation.observe()?;
            if next.live_memory_bytes == snapshot.live_memory_bytes
                && next.reserved == ResourceNeed::default()
            {
                held = Some(next);
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    // Closing both directions wakes a blocked sender and the real peer watch.
    stream.shutdown(Shutdown::Both)?;
    let _sender_outcome = writer.join().map_err(|_panic| "pressure sender panicked")?;
    drop(stream);
    let held = held.ok_or("no data-only response backing observed under real socket pressure")?;
    assert!(held.live_memory_bytes > 0);
    let released_by = Instant::now()
        .checked_add(Duration::from_secs(5))
        .ok_or("release deadline overflow")?;
    while observation.observe()?.live_memory_bytes != 0 && Instant::now() < released_by {
        std::thread::sleep(Duration::from_millis(10));
    }
    let released = observation.observe()?;
    assert_eq!(released.live_memory_bytes, 0);
    assert_eq!(released.retained_memory_bytes, 0);
    assert_eq!(released.pending_escape_bytes, 0);
    assert_eq!(released.reserved, ResourceNeed::default());
    running.stop()
}
