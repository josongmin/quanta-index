//! In-process Fast/Deep execution over one real native service and pinned view.
#![expect(
    clippy::panic_in_result_fn,
    reason = "fixed integration assertions are independent behavior oracles"
)]

#[path = "native_effort/auto_overlay_live.rs"]
mod auto_overlay_live;
#[path = "native_effort/mixed_pressure.rs"]
mod mixed_pressure;
#[path = "common/native_physical_profile.rs"]
mod native_physical_profile;
#[path = "native_effort/overlay.rs"]
mod overlay;
#[path = "native_effort/shared_corpus.rs"]
mod shared_corpus;

use std::{
    error::Error,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        mpsc,
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

use quanta_index_catalog::CATALOG_FILE_NAME;
use quanta_index_contract::{
    EmbeddingNormalization, ExactRepoRelativePathV1, QueryConstraintSetV1, SemanticQueryRequest,
    TextQueryRequest, TextQuerySyntax,
    index::{
        CollectionId, CoverageStatus, Digest, GroupActivation, IndexInstanceId, IndexKind,
        IndexProfile, InputDomain, InstanceSpec, ObjectRef, OperationKey, OperationStatus,
        OwnerKey, OwnerMutation, ProducerId, PublicationGroupId, SchemaId, SlotId, SourceEvent,
        SourceInput, SourceTextAdmission, SourceTransition, StableOwnerId, StorageFormatId,
        StreamId, SubmitOutcome, TenantId, UpdateKind, UpdateRequest, ViewRef, ViewSelection,
        WorkspaceKey, WorktreeId,
    },
    native_protocol::{NativeEffortPreference, NativeIndexRequest, NativeIndexResponse},
};
use quanta_index_core::index::{
    IndexError, IndexQueryPort, QueryCoverage, QueryOutcome, QueryReadContext, WorkBudget,
    WorkLimits,
    effort::{
        DecisionReason, DeclaredCostEstimator, EffortPreference, EffortSignals, HysteresisConfig,
        HysteresisPolicy, PressureThresholds, ProviderAvailability, ProviderMode, ProviderSignal,
        ResourceNeed, Signal, SignalSample, UnknownReason,
    },
};
use quanta_index_core::{CancelHandleV1, CoreError, RequestBudgetV1, TextEmbeddingProvider};
use quanta_index_kernel::{AdapterDescriptor, AdapterRegistry, QueryAdapterRegistry};
use quanta_index_lexical::native::{
    Bm25Policy, NativeTextIndex, NativeTextReader, PlannedTextRead, TextReadResult, TextRecipe,
};
use quanta_index_sdk::native_uds::{
    NativeEffortStreamQuery, NativeEffortStreamStage, NativeUdsClient, NativeUdsError,
};
use quanta_index_searchd_runtime::native::{
    NativeIndexService, NativeResourcePlan, NativeServiceCapabilities, NativeServiceResources,
};
use quanta_index_searchd_runtime::native_effort::{
    NativeEffortClockPort, NativeEffortConfiguration, NativeEffortControl,
    NativeEffortObservationHistory, NativeEffortObserver, NativeEffortRequest,
    NativeEffortSignalPort, NativeEffortStage, NativeEffortSummary,
};
use quanta_index_semantic::native::{
    NativeSemantic, NativeSemanticReadConfig, NativeSemanticReader, SearchResult,
    SemanticReadRequest, TextEmbeddingBridge,
};
use quanta_index_storage::FileObjectStore;

type TestResult<T = ()> = Result<T, Box<dyn Error>>;
const PRODUCER: ProducerId = ProducerId([81; 16]);
const TEXT_INSTANCE: IndexInstanceId = IndexInstanceId([82; 16]);
const SEMANTIC_INSTANCE: IndexInstanceId = IndexInstanceId([83; 16]);
const ADMISSION: Digest = Digest([84; 32]);

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

fn work_operation(sequence: u64) -> OperationKey {
    OperationKey {
        producer: ProducerId([91; 16]),
        stream: StreamId([92; 16]),
        sequence,
        ..operation(1)
    }
}

fn owner() -> OwnerKey {
    OwnerKey {
        producer: PRODUCER,
        stable_id: StableOwnerId([6; 16]),
    }
}

fn text_spec() -> InstanceSpec {
    let recipe = TextRecipe::new(ADMISSION);
    InstanceSpec {
        instance: TEXT_INSTANCE,
        kind: IndexKind([61; 16]),
        group: PublicationGroupId([62; 16]),
        activation: GroupActivation::CoreRequired,
        input_domain: InputDomain::SourceLocal,
        schema: SchemaId([63; 16]),
        policy: recipe.output_policy(),
        format: StorageFormatId([64; 16]),
    }
}

fn semantic_spec(recipe: &quanta_index_semantic::native::EmbeddingRecipe) -> InstanceSpec {
    InstanceSpec {
        instance: SEMANTIC_INSTANCE,
        kind: IndexKind([71; 16]),
        group: PublicationGroupId([72; 16]),
        activation: GroupActivation::AsyncCoverage,
        input_domain: InputDomain::SourceLocal,
        schema: SchemaId([73; 16]),
        policy: recipe.fingerprint(),
        format: StorageFormatId([74; 16]),
    }
}

fn limits() -> WorkLimits {
    WorkLimits {
        reads: 1_000_000,
        writes: 1_000_000,
        bytes: 128 * 1024 * 1024,
        mutations: 1_000_000,
    }
}

fn budget() -> RequestBudgetV1 {
    RequestBudgetV1::for_duration(Duration::from_secs(30))
}

fn published(outcome: &SubmitOutcome) -> TestResult<ViewRef> {
    let SubmitOutcome::Terminal(OperationStatus::Published(view)) = outcome else {
        return Err(format!("publication failed: {outcome:?}").into());
    };
    Ok(*view)
}

struct FixedProvider {
    calls: Arc<AtomicUsize>,
    gate: Option<Arc<ProviderGate>>,
}

struct ProviderGate {
    entered: AtomicBool,
    released: AtomicBool,
    max_wait: Duration,
    cooperative: bool,
}

impl Default for ProviderGate {
    fn default() -> Self {
        Self {
            entered: AtomicBool::new(false),
            released: AtomicBool::new(false),
            max_wait: Duration::from_secs(3),
            cooperative: true,
        }
    }
}

impl TextEmbeddingProvider for FixedProvider {
    fn embed_batch(
        &self,
        texts: &[&str],
    ) -> Result<quanta_index_core::TextEmbeddingBatch, CoreError> {
        texts
            .iter()
            .map(|text| match *text {
                "alpha" | "query" | "row-0" => Ok(vec![1.0, 0.0]),
                "beta" | "row-1" | "row gap 0" => Ok(vec![0.0, 1.0]),
                other => Err(CoreError::InvalidContract(format!(
                    "unexpected text: {other}"
                ))),
            })
            .collect()
    }
    fn embed_batch_within(
        &self,
        texts: &[&str],
        request: &RequestBudgetV1,
    ) -> Result<quanta_index_core::TextEmbeddingBatch, CoreError> {
        request.checkpoint("effort:embed")?;
        let previous = self.calls.fetch_add(1, Ordering::SeqCst);
        if previous > 0
            && let Some(gate) = &self.gate
        {
            gate.entered.store(true, Ordering::Release);
            let started = Instant::now();
            while !gate.released.load(Ordering::Acquire) {
                if gate.cooperative {
                    request.checkpoint("effort:provider-gate")?;
                }
                if started.elapsed() > gate.max_wait {
                    return Err(CoreError::InvalidContract("provider gate timed out".into()));
                }
                std::thread::yield_now();
            }
        }
        let memory = quanta_index_core::TextEmbeddingBatch::reserve_output(
            texts.len(),
            self.dimension(),
            request,
        )?;
        let vectors = self.embed_batch(texts)?.into_unscoped()?;
        Ok(quanta_index_core::TextEmbeddingBatch::from_parts(
            vectors, memory,
        ))
    }
    fn model_id(&self) -> &'static str {
        "effort-fixed-model"
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

fn resources() -> NativeServiceResources {
    let ordinary = ResourceNeed {
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
                provider_units: 2,
            },
            control: ordinary,
            interactive: ResourceNeed {
                provider_units: 1,
                ..ordinary
            },
            core: ordinary,
            enrichment: ResourceNeed {
                provider_units: 1,
                ..ordinary
            },
        },
    }
}

fn service(
    root: &Path,
    socket: &Path,
    calls: Arc<AtomicUsize>,
) -> TestResult<(NativeIndexService, InstanceSpec)> {
    service_with_options(root, socket, calls, ServiceOptions::default())
}

#[derive(Clone)]
struct ServiceOptions {
    malformed_text: bool,
    malformed_semantic: Option<MalformedSemanticMode>,
    semantic_reader: bool,
    resources: NativeServiceResources,
    semantic_need: ResourceNeed,
    provider_gate: Option<Arc<ProviderGate>>,
    semantic_prepare_change: Option<Arc<AtomicBool>>,
}

impl Default for ServiceOptions {
    fn default() -> Self {
        let resources = resources();
        Self {
            malformed_text: false,
            malformed_semantic: None,
            semantic_reader: true,
            semantic_need: resources.execution.enrichment,
            resources,
            provider_gate: None,
            semantic_prepare_change: None,
        }
    }
}

struct MalformedCardinalityReader(NativeTextReader);

impl IndexQueryPort<PlannedTextRead, TextReadResult> for MalformedCardinalityReader {
    fn validate_instance(&self, instance: &InstanceSpec) -> Result<(), IndexError> {
        self.0.validate_instance(instance)
    }

    fn reservation(&self, request: &PlannedTextRead) -> Result<ResourceNeed, IndexError> {
        self.0.reservation(request)
    }

    fn query(
        &self,
        request: &PlannedTextRead,
        context: &mut QueryReadContext<'_>,
    ) -> Result<QueryOutcome<TextReadResult>, IndexError> {
        let mut result = self.0.query(request, context)?.into_result();
        // The adapter retains a valid ticket/view/coverage but violates the
        // public full-match cardinality contract after its real BM25 scan.
        result.matched = 0;
        QueryOutcome::core_scanned(context.ticket, context.instance, true, result)
    }
}

#[derive(Clone, Copy)]
enum MalformedSemanticMode {
    DuplicateIdentity,
    NoFreshScan,
    CappedExhaustive,
    ApproximateExhaustive { capped: bool },
}

struct MalformedSemanticReader(NativeSemanticReader<FixedProvider>, MalformedSemanticMode);

struct ChangeSignalsOnSemanticPrepare(NativeSemanticReader<FixedProvider>, Arc<AtomicBool>);

impl IndexQueryPort<SemanticReadRequest, SearchResult> for ChangeSignalsOnSemanticPrepare {
    fn validate_instance(&self, instance: &InstanceSpec) -> Result<(), IndexError> {
        self.0.validate_instance(instance)
    }

    fn reservation(&self, request: &SemanticReadRequest) -> Result<ResourceNeed, IndexError> {
        let need = self.0.reservation(request)?;
        self.1.store(false, Ordering::SeqCst);
        Ok(need)
    }

    fn query(
        &self,
        request: &SemanticReadRequest,
        context: &mut QueryReadContext<'_>,
    ) -> Result<QueryOutcome<SearchResult>, IndexError> {
        self.0.query(request, context)
    }
}

impl IndexQueryPort<SemanticReadRequest, SearchResult> for MalformedSemanticReader {
    fn validate_instance(&self, instance: &InstanceSpec) -> Result<(), IndexError> {
        self.0.validate_instance(instance)
    }

    fn reservation(&self, request: &SemanticReadRequest) -> Result<ResourceNeed, IndexError> {
        self.0.reservation(request)
    }

    fn query(
        &self,
        request: &SemanticReadRequest,
        context: &mut QueryReadContext<'_>,
    ) -> Result<QueryOutcome<SearchResult>, IndexError> {
        let outcome = self.0.query(request, context)?;
        let QueryCoverage::Async(coverage) = outcome.coverage() else {
            return Err(IndexError::BackendViolation(
                "semantic fixture did not return async coverage".into(),
            ));
        };
        let coverage = **coverage;
        let exhaustive = outcome.exhaustive();
        let mut result = outcome.into_result();
        match self.1 {
            MalformedSemanticMode::DuplicateIdentity => {
                // Deliberately violate only identity, with a newly admitted copy.
                let first = result.hits.first().ok_or_else(|| {
                    IndexError::BackendViolation("semantic fixture hit absent".into())
                })?;
                let memory = context.budget.reserve_memory(
                    u64::try_from(first.vector_id.len())
                        .map_err(|_error| IndexError::ResourceLimited)?,
                    quanta_index_core::index::MemoryLifetime::Retained,
                )?;
                let mut id = Vec::new();
                id.try_reserve_exact(first.vector_id.len())
                    .map_err(|_error| IndexError::ResourceLimited)?;
                id.extend_from_slice(&first.vector_id);
                let copied = quanta_index_semantic::native::SearchHit::from_parts(
                    first.owner,
                    id,
                    first.cosine,
                    memory,
                );
                let new_count = result
                    .hits
                    .len()
                    .checked_add(1)
                    .ok_or(IndexError::ResourceLimited)?;
                let bytes = new_count
                    .checked_mul(std::mem::size_of::<quanta_index_semantic::native::SearchHit>())
                    .ok_or(IndexError::ResourceLimited)?;
                let capacity = context.budget.reserve_memory(
                    u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)?,
                    quanta_index_core::index::MemoryLifetime::Retained,
                )?;
                let mut duplicated = SearchResult::from_parts(
                    Vec::new(),
                    result.scanned_owners,
                    result.skipped_stale,
                    result.traversal,
                    result.exhaustive,
                    capacity,
                );
                duplicated
                    .hits
                    .try_reserve_exact(new_count)
                    .map_err(|_error| IndexError::ResourceLimited)?;
                duplicated.hits.append(&mut result.hits);
                duplicated.hits.push(copied);
                result = duplicated;
            }
            MalformedSemanticMode::NoFreshScan => result.scanned_owners = 0,
            MalformedSemanticMode::CappedExhaustive => {
                assert!(exhaustive && result.exhaustive);
                assert_eq!(
                    result.traversal,
                    quanta_index_contract::native_protocol::NativeSemanticTraversal::ExactExhausted
                );
                result.traversal =
                    quanta_index_contract::native_protocol::NativeSemanticTraversal::ExactCapped;
            }
            MalformedSemanticMode::ApproximateExhaustive { capped } => {
                assert!(exhaustive && result.exhaustive);
                assert_eq!(
                    result.traversal,
                    quanta_index_contract::native_protocol::NativeSemanticTraversal::ExactExhausted
                );
                result.traversal =
                    quanta_index_contract::native_protocol::NativeSemanticTraversal::Approximate {
                        capped,
                    };
            }
        }
        QueryOutcome::async_ranked(
            context.ticket,
            context.instance,
            &coverage,
            exhaustive,
            result,
        )
    }
}

fn service_with_options(
    root: &Path,
    socket: &Path,
    calls: Arc<AtomicUsize>,
    options: ServiceOptions,
) -> TestResult<(NativeIndexService, InstanceSpec)> {
    let bridge = Arc::new(TextEmbeddingBridge::new(FixedProvider {
        calls,
        gate: options.provider_gate,
    })?);
    let recipe = bridge.recipe(1024)?;
    let semantic_spec = semantic_spec(&recipe);
    let text_spec = text_spec();
    let text_descriptor = AdapterDescriptor {
        kind: text_spec.kind,
        schema: text_spec.schema,
        format: text_spec.format,
    };
    let semantic_descriptor = AdapterDescriptor {
        kind: semantic_spec.kind,
        schema: semantic_spec.schema,
        format: semantic_spec.format,
    };
    let mut builders = AdapterRegistry::new();
    builders.register(
        text_descriptor,
        NativeTextIndex::new(TextRecipe::new(ADMISSION)),
        |delta| Ok(delta.clone()),
    )?;
    builders.register_owner(
        semantic_descriptor,
        NativeSemantic::new(Arc::clone(&bridge), recipe.clone())?,
    )?;
    let text_need = ResourceNeed {
        provider_units: 0,
        ..options.resources.execution.interactive
    };
    let semantic_need = options.semantic_need;
    let mut text = QueryAdapterRegistry::<PlannedTextRead, TextReadResult>::new();
    let text_reader = NativeTextReader::new(
        TextRecipe::new(ADMISSION),
        Bm25Policy { k1: 1.2, b: 0.75 },
        text_need,
    )?;
    if options.malformed_text {
        text.register(
            text_descriptor,
            text_need,
            MalformedCardinalityReader(text_reader),
        )?;
    } else {
        text.register(text_descriptor, text_need, text_reader)?;
    }
    let mut semantic = QueryAdapterRegistry::<SemanticReadRequest, SearchResult>::new();
    if options.semantic_reader {
        let reader = NativeSemanticReader::new(
            bridge,
            recipe,
            NativeSemanticReadConfig {
                page_size: 8,
                max_owners: 8,
                max_query_bytes: 1024,
                need: semantic_need,
            },
        )?;
        if let Some(change) = options.semantic_prepare_change {
            semantic.register(
                semantic_descriptor,
                semantic_need,
                ChangeSignalsOnSemanticPrepare(reader, change),
            )?;
        } else if let Some(mode) = options.malformed_semantic {
            semantic.register(
                semantic_descriptor,
                semantic_need,
                MalformedSemanticReader(reader, mode),
            )?;
        } else {
            semantic.register(semantic_descriptor, semantic_need, reader)?;
        }
    }
    Ok((
        NativeIndexService::bind_with_capabilities(
            root,
            socket,
            limits(),
            NativeServiceCapabilities {
                builders,
                literal: QueryAdapterRegistry::new(),
                path: QueryAdapterRegistry::new(),
                text,
                semantic,
                pump: None,
                provider_attempt_pool: None,
            },
            options.resources,
        )?,
        semantic_spec,
    ))
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
                .map_err(|_panic| "native service thread panicked")??;
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

fn setup(client: &NativeUdsClient, semantic: InstanceSpec) -> TestResult<(ViewRef, ObjectRef)> {
    let NativeIndexResponse::Created(genesis) = client.request(
        NativeIndexRequest::CreateWorkspace {
            workspace: workspace(),
            epoch: 1,
        },
        &budget(),
    )?
    else {
        return Err("genesis reply differs".into());
    };
    let ingress = client.begin_input(operation(1), &budget())?;
    let profile = client.stage_profile(
        ingress,
        IndexProfile {
            instances: vec![text_spec(), semantic],
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
    let (pending, source) = advance(client, configured, 2, 1, b"alpha", None)?;
    let NativeIndexResponse::ReadyWork(ready) = client.request(
        NativeIndexRequest::ReadyWork {
            workspace: workspace(),
            epoch: 1,
            limit: 8,
        },
        &budget(),
    )?
    else {
        return Err("work reply differs".into());
    };
    assert_eq!(ready.len(), 1);
    let intent = ready.into_iter().next().ok_or("semantic work absent")?;
    let NativeIndexResponse::Claimed(claim) = client.request(
        NativeIndexRequest::ClaimWork {
            intent,
            lease_millis: 60_000,
        },
        &budget(),
    )?
    else {
        return Err("claim reply differs".into());
    };
    let complete = published(&client.submit(
        UpdateRequest {
            operation: work_operation(1),
            expected_view: pending,
            kind: UpdateKind::Materialize { claim },
        },
        &budget(),
    )?)?;
    Ok((complete, source))
}

fn advance(
    client: &NativeUdsClient,
    expected: ViewRef,
    operation_sequence: u64,
    event_sequence: u64,
    body: &[u8],
    previous: Option<ObjectRef>,
) -> TestResult<(ViewRef, ObjectRef)> {
    let key = operation(operation_sequence);
    let ingress = client.begin_input(key, &budget())?;
    let records = client.stage_sources(
        ingress,
        vec![SourceInput {
            path: "a.rs".into(),
            bytes: body.to_vec(),
            text: SourceTextAdmission::Utf8 { policy: ADMISSION },
        }],
        &budget(),
    )?;
    let record = *records.first().ok_or("staged source absent")?;
    let prior = client.read_view(ViewSelection::Exact(expected), &budget())?;
    let owners = client.stage_mutations(
        ingress,
        [OwnerMutation {
            owner: owner(),
            expected: previous,
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
        producer: PRODUCER,
        stream: key.stream,
        stream_epoch: 1,
        predecessor: event_sequence.checked_sub(1).ok_or("event starts at one")?,
        sequence: event_sequence,
        payload: transition.owner_digest()?,
    });
    let view = published(&client.submit(
        UpdateRequest {
            operation: key,
            expected_view: expected,
            kind: UpdateKind::SourceAdvance(transition),
        },
        &budget(),
    )?)?;
    Ok((view, record))
}

fn text_request() -> TextQueryRequest {
    TextQueryRequest {
        syntax: TextQuerySyntax::Native,
        query_text: "alpha".into(),
        constraints: QueryConstraintSetV1::unconstrained(),
        generation: None,
        generation_selector: None,
        top_k: 1,
        cursor: None,
    }
}

fn semantic_request() -> SemanticQueryRequest {
    SemanticQueryRequest {
        query_text: "query".into(),
        constraints: QueryConstraintSetV1::unconstrained(),
        generation: None,
        generation_selector: None,
        lexical_scope: None,
        top_k: 1,
    }
}

fn request(view: ViewRef, preference: EffortPreference) -> NativeEffortRequest {
    NativeEffortRequest {
        selection: ViewSelection::Exact(view),
        text_instance: TEXT_INSTANCE,
        semantic_instance: SEMANTIC_INSTANCE,
        text_query: text_request(),
        semantic_query: semantic_request(),
        preference,
        overlay: None,
    }
}

fn stream_query(view: ViewRef) -> NativeEffortStreamQuery {
    NativeEffortStreamQuery {
        overlay: None,
        selection: ViewSelection::Exact(view),
        text_instance: TEXT_INSTANCE,
        semantic_instance: SEMANTIC_INSTANCE,
        text_query: text_request(),
        semantic_query: semantic_request(),
        preference: NativeEffortPreference::Deep,
    }
}

fn wait_until(mut ready: impl FnMut() -> bool) -> TestResult {
    let started = Instant::now();
    while !ready() {
        if started.elapsed() > Duration::from_secs(3) {
            return Err("timed out waiting for native stream state".into());
        }
        std::thread::yield_now();
    }
    Ok(())
}

fn unknown_signals() -> EffortSignals {
    EffortSignals {
        interaction_idle_ms: Signal::Unknown(UnknownReason::Missing),
        interactive_pressure_bps: Signal::Unknown(UnknownReason::Missing),
        index_backlog: Signal::Unknown(UnknownReason::Missing),
        dispatch_lag_ms: Signal::Unknown(UnknownReason::Missing),
        memory_headroom_bytes: Signal::Unknown(UnknownReason::Missing),
        provider: Signal::Unknown(UnknownReason::Missing),
        quality_confidence_bps: Signal::Unknown(UnknownReason::Missing),
    }
}

struct UnknownSignals;
impl NativeEffortSignalPort for UnknownSignals {
    fn sample(&self, _now_ms: u64) -> EffortSignals {
        unknown_signals()
    }
}

struct StageObserver<F>(F);

impl<F> NativeEffortObserver for StageObserver<F>
where
    F: for<'a> FnMut(NativeEffortStage<'a>) -> Result<(), IndexError>,
{
    fn observe(&mut self, stage: NativeEffortStage<'_>) -> Result<(), IndexError> {
        (self.0)(stage)
    }
}

fn catalog_path(root: &Path) -> PathBuf {
    root.join("catalog").join(CATALOG_FILE_NAME)
}

fn pin_count(path: &Path) -> Result<i64, IndexError> {
    rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .and_then(|connection| {
            connection.query_row("SELECT COUNT(*) FROM native_pin_v1", [], |row| row.get(0))
        })
        .map_err(|error| IndexError::Storage(error.to_string()))
}

#[derive(Clone, Copy)]
struct TerminalObservation {
    view: ViewRef,
    reason: DecisionReason,
    deep_delivered: bool,
}

struct TerminalCustodyObserver<'a> {
    catalog: &'a Path,
    expected_pins: i64,
    view: ViewRef,
    stages: Vec<u8>,
    terminal: Option<TerminalObservation>,
}

impl NativeEffortObserver for TerminalCustodyObserver<'_> {
    fn observe(&mut self, stage: NativeEffortStage<'_>) -> Result<(), IndexError> {
        if pin_count(self.catalog)? != self.expected_pins {
            return Err(IndexError::BackendViolation("stage pin absent".into()));
        }
        match stage {
            NativeEffortStage::Fast(fast) if fast.view == self.view && self.stages.is_empty() => {
                self.stages.push(1);
            }
            NativeEffortStage::Deep(deep)
                if deep.view == self.view && self.stages.as_slice() == [1] =>
            {
                self.stages.push(2);
            }
            NativeEffortStage::Fast(_)
            | NativeEffortStage::Deep(_)
            | NativeEffortStage::OverlayFast(_)
            | NativeEffortStage::OverlayDeep(_) => {
                return Err(IndexError::BackendViolation(
                    "stage order or view changed".into(),
                ));
            }
        }
        Ok(())
    }

    fn finish(&mut self, summary: NativeEffortSummary<'_>) -> Result<(), IndexError> {
        if pin_count(self.catalog)? != self.expected_pins
            || summary.view != self.view
            || summary.corpus.is_some()
            || self.stages.is_empty()
            || summary.deep_delivered != (self.stages.as_slice() == [1, 2])
        {
            return Err(IndexError::BackendViolation(
                "terminal callback lost selected pin or stage custody".into(),
            ));
        }
        if self
            .terminal
            .replace(TerminalObservation {
                view: summary.view,
                reason: summary.reason,
                deep_delivered: summary.deep_delivered,
            })
            .is_some()
        {
            return Err(IndexError::BackendViolation(
                "duplicate terminal callback".into(),
            ));
        }
        Ok(())
    }
}

struct ClearSignals;
fn known<T>(value: T, observed_at_ms: u64) -> Signal<T> {
    Signal::Known(SignalSample {
        value,
        observed_at_ms,
        valid_for_ms: 1_000,
    })
}
impl NativeEffortSignalPort for ClearSignals {
    fn sample(&self, now_ms: u64) -> EffortSignals {
        EffortSignals {
            interaction_idle_ms: known(500, now_ms),
            interactive_pressure_bps: known(0, now_ms),
            index_backlog: known(0, now_ms),
            dispatch_lag_ms: known(0, now_ms),
            memory_headroom_bytes: known(2 * 1024 * 1024, now_ms),
            provider: known(
                ProviderSignal {
                    availability: ProviderAvailability::Ready,
                    mode: ProviderMode::Local,
                },
                now_ms,
            ),
            quality_confidence_bps: Signal::Unknown(UnknownReason::Missing),
        }
    }
}

struct HighPressureSignals;
impl NativeEffortSignalPort for HighPressureSignals {
    fn sample(&self, now_ms: u64) -> EffortSignals {
        let mut signals = ClearSignals.sample(now_ms);
        signals.interactive_pressure_bps = known(9_000, now_ms);
        signals
    }
}

struct MutableReadinessSignals(Arc<AtomicBool>);

impl NativeEffortSignalPort for MutableReadinessSignals {
    fn sample(&self, now_ms: u64) -> EffortSignals {
        let mut signals = ClearSignals.sample(now_ms);
        if !self.0.load(Ordering::SeqCst) {
            signals.interaction_idle_ms = Signal::Unknown(UnknownReason::Missing);
            signals.provider = Signal::Known(SignalSample {
                value: ProviderSignal {
                    availability: ProviderAvailability::Offline,
                    mode: ProviderMode::Local,
                },
                observed_at_ms: now_ms,
                valid_for_ms: 1_000,
            });
        }
        signals
    }
}

struct TwoCheckpoints(AtomicUsize);
impl NativeEffortClockPort for TwoCheckpoints {
    fn now_ms(&self) -> u64 {
        // Query origin and both reads around the first actual clear sample
        // are 0. Time advances only after that observation was committed.
        if self.0.fetch_add(1, Ordering::SeqCst) < 3 {
            0
        } else {
            500
        }
    }
}

struct ManualEffortClock(AtomicU64);

impl NativeEffortClockPort for ManualEffortClock {
    fn now_ms(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}

struct FreshClearSignals(Arc<AtomicBool>);

fn fresh_clear_signal<T>(value: T, observed_at_ms: u64) -> Signal<T> {
    Signal::Known(SignalSample {
        value,
        observed_at_ms,
        valid_for_ms: 20,
    })
}

impl NativeEffortSignalPort for FreshClearSignals {
    fn sample(&self, now_ms: u64) -> EffortSignals {
        EffortSignals {
            interaction_idle_ms: fresh_clear_signal(500_u64, now_ms),
            interactive_pressure_bps: fresh_clear_signal(
                if self.0.load(Ordering::SeqCst) {
                    0_u16
                } else {
                    9_000_u16
                },
                now_ms,
            ),
            index_backlog: fresh_clear_signal(0_u32, now_ms),
            dispatch_lag_ms: fresh_clear_signal(0_u64, now_ms),
            memory_headroom_bytes: fresh_clear_signal(2 * 1024 * 1024_u64, now_ms),
            provider: fresh_clear_signal(
                ProviderSignal {
                    availability: ProviderAvailability::Ready,
                    mode: ProviderMode::Local,
                },
                now_ms,
            ),
            quality_confidence_bps: Signal::Unknown(UnknownReason::Missing),
        }
    }
}

struct CancelAfterFast {
    calls: AtomicUsize,
    cancel: CancelHandleV1,
}
impl NativeEffortSignalPort for CancelAfterFast {
    fn sample(&self, _now_ms: u64) -> EffortSignals {
        if self.calls.fetch_add(1, Ordering::SeqCst) == 1 {
            self.cancel.cancel();
        }
        unknown_signals()
    }
}

fn policy() -> TestResult<HysteresisPolicy> {
    policy_with_dwell(1, 1, 0)
}

fn policy_with_dwell(
    stable_idle_ms: u64,
    clear_dwell_ms: u64,
    minimum_residence_ms: u64,
) -> TestResult<HysteresisPolicy> {
    Ok(HysteresisPolicy::new(HysteresisConfig {
        thresholds: PressureThresholds {
            interactive_low_bps: 1000,
            interactive_high_bps: 8000,
            backlog_low: 1,
            backlog_high: 100,
            lag_low_ms: 10,
            lag_high_ms: 1000,
            memory_high_pressure_bytes: 1024,
            memory_clear_bytes: 1024 * 1024,
        },
        stable_idle_ms,
        clear_dwell_ms,
        minimum_residence_ms,
    })?)
}

#[test]
fn in_process_fast_auto_unknown_and_deep_use_real_old_view() -> TestResult {
    let root = tempfile::tempdir()?;
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))?;
    let socket = root.path().join("native.sock");
    let calls = Arc::new(AtomicUsize::new(0));
    let (service, semantic) = service(root.path(), &socket, Arc::clone(&calls))?;
    let handle = service.effort_handle();
    let running = RunningService::start(service);
    let client = NativeUdsClient::new(socket);
    let (view_a, a_record) = setup(&client, semantic)?;
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let (view_b, b_record) = advance(&client, view_a, 4, 2, b"beta", Some(a_record))?;
    assert_ne!(view_a, view_b);
    assert_ne!(a_record, b_record);
    let policy = policy()?;
    let estimator = DeclaredCostEstimator;
    let signals = UnknownSignals;
    for preference in [EffortPreference::Fast, EffortPreference::Auto] {
        let mut work = WorkBudget::with_request(limits(), budget());
        let control = NativeEffortControl {
            policy: &policy,
            estimator: &estimator,
            signals: &signals,
            clock: None,
            history: None,
        };
        let result = handle.query_effort(request(view_a, preference), control, &mut work)?;
        assert_eq!(result.view, view_a);
        assert_eq!(result.fast.durable()?.view, view_a);
        assert_eq!(result.fast.durable()?.hits.len(), 1);
        assert_eq!(
            result
                .fast
                .durable()?
                .hits
                .first()
                .ok_or("Fast hit absent")?
                .source,
            a_record
        );
        assert!(result.deep.is_none());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
    let mut work = WorkBudget::with_request(limits(), budget());
    let control = NativeEffortControl {
        policy: &policy,
        estimator: &estimator,
        signals: &signals,
        clock: None,
        history: None,
    };
    let result =
        handle.query_effort(request(view_a, EffortPreference::Deep), control, &mut work)?;
    assert_eq!(result.view, view_a);
    assert_eq!(
        result
            .fast
            .durable()?
            .hits
            .first()
            .ok_or("Deep Fast hit absent")?
            .source,
        a_record
    );
    let deep = result
        .deep
        .ok_or("Deep preference did not execute semantic stage")?
        .into_durable()?;
    assert_eq!(deep.view, view_a);
    assert_eq!(deep.coverage.status, CoverageStatus::Complete);
    assert_eq!(deep.coverage.covered, Some(deep.coverage.desired));
    assert!(deep.exhaustive);
    assert_eq!(deep.hits.len(), 1);
    let hit = deep.hits.first().ok_or("Deep hit absent")?;
    assert_eq!(hit.owner, owner());
    assert_eq!(hit.score.get().to_bits(), 1.0f64.to_bits());
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    running.stop()?;
    Ok(())
}

#[test]
fn fast_deep_folded_content_keeps_exact_path_and_selected_old_view() -> TestResult {
    let root = tempfile::tempdir()?;
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))?;
    let socket = root.path().join("native.sock");
    let calls = Arc::new(AtomicUsize::new(0));
    let (service, semantic) = service(root.path(), &socket, Arc::clone(&calls))?;
    let handle = service.effort_handle();
    let running = RunningService::start(service);
    let client = NativeUdsClient::new(socket);
    let (old_view, old_source) = setup(&client, semantic)?;
    let (current_view, current_source) =
        advance(&client, old_view, 4, 2, b"beta", Some(old_source))?;
    assert_ne!(old_view, current_view);
    assert_ne!(old_source, current_source);
    let policy = policy()?;
    let estimator = DeclaredCostEstimator;
    let signals = UnknownSignals;

    // Default Content matching folds case; the canonical path remains byte-exact.
    let mut wrong_case = request(old_view, EffortPreference::Fast);
    wrong_case.text_query.query_text = "ALPHA".into();
    wrong_case.text_query.constraints.repo_relative_path_exact =
        Some(ExactRepoRelativePathV1::new("A.rs")?);
    let mut work = WorkBudget::with_request(limits(), budget());
    let missed = handle.query_effort(
        wrong_case,
        NativeEffortControl {
            policy: &policy,
            estimator: &estimator,
            signals: &signals,
            clock: None,
            history: None,
        },
        &mut work,
    )?;
    assert_eq!(missed.view, old_view);
    assert_eq!(missed.fast.durable()?.view, old_view);
    assert_eq!(missed.fast.durable()?.matched, 0);
    assert!(missed.fast.durable()?.hits.is_empty());
    assert!(missed.deep.is_none());
    drop(missed);

    let mut exact = request(old_view, EffortPreference::Deep);
    exact.text_query.query_text = "ALPHA".into();
    exact.text_query.constraints.repo_relative_path_exact =
        Some(ExactRepoRelativePathV1::new("a.rs")?);
    let mut work = WorkBudget::with_request(limits(), budget());
    let found = handle.query_effort(
        exact,
        NativeEffortControl {
            policy: &policy,
            estimator: &estimator,
            signals: &signals,
            clock: None,
            history: None,
        },
        &mut work,
    )?;
    assert_eq!(found.view, old_view);
    assert_eq!(found.fast.durable()?.view, old_view);
    assert_eq!(found.fast.durable()?.matched, 1);
    assert_eq!(found.fast.durable()?.hits.len(), 1);
    let hit = found
        .fast
        .durable()?
        .hits
        .first()
        .ok_or("exact Fast hit absent")?;
    assert_eq!(hit.owner, owner());
    assert_eq!(hit.source, old_source);
    assert_eq!(hit.path, "a.rs");
    let deep = found.deep.ok_or("Deep result absent")?.into_durable()?;
    assert_eq!(deep.view, old_view);
    assert_eq!(deep.coverage.status, CoverageStatus::Complete);
    assert_eq!(deep.hits.len(), 1);
    assert_eq!(deep.hits.first().ok_or("Deep hit absent")?.owner, owner());
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    running.stop()?;
    Ok(())
}

#[test]
fn auto_clear_dwell_runs_optional_deep_on_the_same_pinned_view() -> TestResult {
    let root = tempfile::tempdir()?;
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))?;
    let socket = root.path().join("native.sock");
    let calls = Arc::new(AtomicUsize::new(0));
    let (service, semantic) = service(root.path(), &socket, Arc::clone(&calls))?;
    let handle = service.effort_handle();
    let running = RunningService::start(service);
    let client = NativeUdsClient::new(socket);
    let (view, _) = setup(&client, semantic)?;
    let policy = policy_with_dwell(100, 200, 100)?;
    let estimator = DeclaredCostEstimator;
    let signals = ClearSignals;
    let clock = TwoCheckpoints(AtomicUsize::new(0));
    let mut work = WorkBudget::with_request(limits(), budget());
    let result = handle.query_effort(
        request(view, EffortPreference::Auto),
        NativeEffortControl {
            policy: &policy,
            estimator: &estimator,
            signals: &signals,
            clock: Some(&clock),
            history: None,
        },
        &mut work,
    )?;
    assert_eq!(result.view, view);
    assert_eq!(result.fast.durable()?.view, view);
    let deep = result
        .deep
        .ok_or("Auto did not execute Deep after clear dwell")?
        .into_durable()?;
    assert_eq!(deep.view, view);
    assert_eq!(deep.coverage.status, CoverageStatus::Complete);
    assert!(deep.exhaustive);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    let signal_ports: [&dyn NativeEffortSignalPort; 2] = [&UnknownSignals, &HighPressureSignals];
    for signals in signal_ports {
        let clock = TwoCheckpoints(AtomicUsize::new(0));
        let mut work = WorkBudget::with_request(limits(), budget());
        let result = handle.query_effort(
            request(view, EffortPreference::Auto),
            NativeEffortControl {
                policy: &policy,
                estimator: &estimator,
                signals,
                clock: Some(&clock),
                history: None,
            },
            &mut work,
        )?;
        assert_eq!(result.fast.durable()?.view, view);
        assert!(result.deep.is_none());
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }
    running.stop()?;
    Ok(())
}

#[test]
fn auto_rechecks_interaction_and_provider_after_optional_prepare() -> TestResult {
    let root = tempfile::tempdir()?;
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))?;
    let socket = root.path().join("native.sock");
    let calls = Arc::new(AtomicUsize::new(0));
    let ready = Arc::new(AtomicBool::new(true));
    let (service, semantic) = service_with_options(
        root.path(),
        &socket,
        Arc::clone(&calls),
        ServiceOptions {
            semantic_prepare_change: Some(Arc::clone(&ready)),
            ..ServiceOptions::default()
        },
    )?;
    let handle = service.effort_handle();
    let running = RunningService::start(service);
    let client = NativeUdsClient::new(socket);
    let (view, _) = setup(&client, semantic)?;
    let calls_before = calls.load(Ordering::SeqCst);
    let policy = policy_with_dwell(100, 200, 100)?;
    let estimator = DeclaredCostEstimator;
    let signals = MutableReadinessSignals(Arc::clone(&ready));
    let clock = TwoCheckpoints(AtomicUsize::new(0));
    let mut work = WorkBudget::with_request(limits(), budget());
    let result = handle.query_effort(
        request(view, EffortPreference::Auto),
        NativeEffortControl {
            policy: &policy,
            estimator: &estimator,
            signals: &signals,
            clock: Some(&clock),
            history: None,
        },
        &mut work,
    )?;
    assert!(!ready.load(Ordering::SeqCst));
    assert_eq!(result.fast.durable()?.view, view);
    assert!(result.deep.is_none());
    assert_eq!(calls.load(Ordering::SeqCst), calls_before);
    running.stop()?;
    Ok(())
}

#[test]
fn shared_auto_observation_accumulates_only_contiguous_fresh_windows() -> TestResult {
    let root = tempfile::tempdir()?;
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))?;
    let socket = root.path().join("native.sock");
    let calls = Arc::new(AtomicUsize::new(0));
    let (service, semantic) = service(root.path(), &socket, Arc::clone(&calls))?;
    let handle = service.effort_handle();
    let running = RunningService::start(service);
    let client = NativeUdsClient::new(socket);
    let (view, _) = setup(&client, semantic)?;
    let clock = Arc::new(ManualEffortClock(AtomicU64::new(0)));
    let observation_clock: Arc<dyn NativeEffortClockPort> = clock.clone();
    let history = NativeEffortObservationHistory::with_clock(observation_clock);
    let clear = Arc::new(AtomicBool::new(true));
    let signals = FreshClearSignals(Arc::clone(&clear));
    let policy = policy_with_dwell(100, 100, 100)?;
    let estimator = DeclaredCostEstimator;
    let control = NativeEffortControl {
        policy: &policy,
        estimator: &estimator,
        signals: &signals,
        clock: None,
        history: Some(&history),
    };
    for at_ms in [0, 20, 40, 60, 80, 100] {
        clock.0.store(at_ms, Ordering::SeqCst);
        let preference = if at_ms == 80 {
            EffortPreference::Fast
        } else {
            EffortPreference::Auto
        };
        let mut work = WorkBudget::with_request(limits(), budget());
        let result = handle.query_effort(request(view, preference), control, &mut work)?;
        assert_eq!(result.fast.durable()?.view, view);
        assert_eq!(result.deep.is_some(), at_ms == 100);
    }
    // The previous delivery reset Fast residence. A 100 ms gap also invalidates
    // the old clear window even though the current sample is fresh.
    clock.0.store(200, Ordering::SeqCst);
    let mut work = WorkBudget::with_request(limits(), budget());
    let after_gap =
        handle.query_effort(request(view, EffortPreference::Auto), control, &mut work)?;
    assert!(after_gap.deep.is_none());
    clear.store(false, Ordering::SeqCst);
    clock.0.store(220, Ordering::SeqCst);
    let mut work = WorkBudget::with_request(limits(), budget());
    let under_pressure =
        handle.query_effort(request(view, EffortPreference::Auto), control, &mut work)?;
    assert_eq!(under_pressure.reason, DecisionReason::Pressure);
    assert!(under_pressure.deep.is_none());
    clear.store(true, Ordering::SeqCst);
    for at_ms in [240, 260, 280, 300, 320, 340] {
        clock.0.store(at_ms, Ordering::SeqCst);
        let mut work = WorkBudget::with_request(limits(), budget());
        let result =
            handle.query_effort(request(view, EffortPreference::Auto), control, &mut work)?;
        assert_eq!(result.deep.is_some(), at_ms == 340);
    }
    assert_eq!(calls.load(Ordering::SeqCst), 3);
    clock.0.store(339, Ordering::SeqCst);
    let mut work = WorkBudget::with_request(limits(), budget());
    assert!(matches!(
        handle.query_effort(request(view, EffortPreference::Auto), control, &mut work),
        Err(IndexError::BackendViolation(_))
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 3);
    let mut work = WorkBudget::with_request(limits(), budget());
    assert!(matches!(
        handle.query_effort(
            request(view, EffortPreference::Auto),
            NativeEffortControl {
                clock: Some(clock.as_ref()),
                ..control
            },
            &mut work,
        ),
        Err(IndexError::InvalidInput(_))
    ));
    running.stop()?;
    Ok(())
}

#[test]
fn malformed_fast_adapter_receipt_cannot_be_promoted_to_proof() -> TestResult {
    let root = tempfile::tempdir()?;
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))?;
    let socket = root.path().join("native.sock");
    let calls = Arc::new(AtomicUsize::new(0));
    let (service, semantic) = service_with_options(
        root.path(),
        &socket,
        Arc::clone(&calls),
        ServiceOptions {
            malformed_text: true,
            ..ServiceOptions::default()
        },
    )?;
    let handle = service.effort_handle();
    let running = RunningService::start(service);
    let client = NativeUdsClient::new(socket);
    let (view, _) = setup(&client, semantic)?;
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let policy = policy()?;
    let estimator = DeclaredCostEstimator;
    let signals = UnknownSignals;
    let mut work = WorkBudget::with_request(limits(), budget());
    assert!(matches!(
        handle.query_effort(
            request(view, EffortPreference::Fast),
            NativeEffortControl {
                policy: &policy,
                estimator: &estimator,
                signals: &signals,
                clock: None,
                history: None,
            },
            &mut work,
        ),
        Err(IndexError::BackendViolation(_))
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    running.stop()?;
    Ok(())
}

#[test]
fn auto_without_configured_semantic_reader_keeps_fast_result() -> TestResult {
    let root = tempfile::tempdir()?;
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))?;
    let socket = root.path().join("native.sock");
    let calls = Arc::new(AtomicUsize::new(0));
    let (service, semantic) = service_with_options(
        root.path(),
        &socket,
        Arc::clone(&calls),
        ServiceOptions {
            semantic_reader: false,
            ..ServiceOptions::default()
        },
    )?;
    let handle = service.effort_handle();
    let running = RunningService::start(service);
    let client = NativeUdsClient::new(socket);
    let (view, _) = setup(&client, semantic)?;
    let baseline = calls.load(Ordering::SeqCst);
    let policy = policy_with_dwell(100, 200, 100)?;
    let estimator = DeclaredCostEstimator;
    let signals = ClearSignals;
    let clock = TwoCheckpoints(AtomicUsize::new(0));
    let mut work = WorkBudget::with_request(limits(), budget());
    let result = handle.query_effort(
        request(view, EffortPreference::Auto),
        NativeEffortControl {
            policy: &policy,
            estimator: &estimator,
            signals: &signals,
            clock: Some(&clock),
            history: None,
        },
        &mut work,
    )?;
    assert_eq!(result.fast.durable()?.view, view);
    assert!(result.deep.is_none());
    assert_eq!(result.reason, DecisionReason::ProviderUnavailable);
    assert_eq!(calls.load(Ordering::SeqCst), baseline);
    running.stop()?;
    Ok(())
}

#[test]
fn optional_deep_refusal_preserves_fast_when_enrichment_envelope_is_smaller() -> TestResult {
    let root = tempfile::tempdir()?;
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))?;
    let socket = root.path().join("native.sock");
    let calls = Arc::new(AtomicUsize::new(0));
    let mut options = ServiceOptions::default();
    options.resources.execution.capacity.cpu_units = 6;
    options.resources.execution.interactive.cpu_units = 2;
    options.semantic_need = options.resources.execution.interactive;
    let (service, semantic) =
        service_with_options(root.path(), &socket, Arc::clone(&calls), options)?;
    let handle = service.effort_handle();
    let running = RunningService::start(service);
    let client = NativeUdsClient::new(socket);
    let (view, _) = setup(&client, semantic)?;
    let baseline = calls.load(Ordering::SeqCst);
    let policy = policy()?;
    let estimator = DeclaredCostEstimator;
    let signals = UnknownSignals;
    let mut work = WorkBudget::with_request(limits(), budget());
    let result = handle.query_effort(
        request(view, EffortPreference::Deep),
        NativeEffortControl {
            policy: &policy,
            estimator: &estimator,
            signals: &signals,
            clock: None,
            history: None,
        },
        &mut work,
    )?;
    assert_eq!(result.fast.durable()?.view, view);
    assert!(result.deep.is_none());
    assert_eq!(result.reason, DecisionReason::ResourceDenied);
    assert_eq!(calls.load(Ordering::SeqCst), baseline);
    running.stop()?;
    Ok(())
}

#[test]
fn duplicate_semantic_identity_cannot_escape_the_in_process_controller() -> TestResult {
    let root = tempfile::tempdir()?;
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))?;
    let socket = root.path().join("native.sock");
    let calls = Arc::new(AtomicUsize::new(0));
    let (service, semantic) = service_with_options(
        root.path(),
        &socket,
        Arc::clone(&calls),
        ServiceOptions {
            malformed_semantic: Some(MalformedSemanticMode::DuplicateIdentity),
            ..ServiceOptions::default()
        },
    )?;
    let observation = service.resource_observation();
    let handle = service.effort_handle();
    let running = RunningService::start(service);
    let client = NativeUdsClient::new(socket);
    let (view, _) = setup(&client, semantic)?;
    let catalog_path = catalog_path(root.path());
    let initial_pins = pin_count(&catalog_path)?;
    let baseline_calls = calls.load(Ordering::SeqCst);
    let policy = policy()?;
    let estimator = DeclaredCostEstimator;
    let signals = UnknownSignals;
    let mut request = request(view, EffortPreference::Deep);
    request.semantic_query.top_k = 2;
    let mut work = WorkBudget::with_request(limits(), budget());
    assert!(matches!(
        handle.query_effort(
            request,
            NativeEffortControl {
                policy: &policy,
                estimator: &estimator,
                signals: &signals,
                clock: None,
                history: None,
            },
            &mut work,
        ),
        Err(IndexError::BackendViolation(_))
    ));
    assert_eq!(calls.load(Ordering::SeqCst), baseline_calls + 1);
    assert_eq!(pin_count(&catalog_path)?, initial_pins);
    let resources = observation.observe()?;
    assert_eq!(resources.reserved, ResourceNeed::default());
    assert_eq!(resources.interactive.active, 0);
    assert_eq!(resources.enrichment.active, 0);
    running.stop()?;
    Ok(())
}

#[test]
fn semantic_hit_without_a_fresh_scanned_owner_cannot_escape_the_controller() -> TestResult {
    rejected_semantic_metadata_refunds(MalformedSemanticMode::NoFreshScan)
}

#[test]
fn capped_or_approximate_exhaustion_cannot_escape_the_controller() -> TestResult {
    for mode in [
        MalformedSemanticMode::CappedExhaustive,
        MalformedSemanticMode::ApproximateExhaustive { capped: false },
        MalformedSemanticMode::ApproximateExhaustive { capped: true },
    ] {
        rejected_semantic_metadata_refunds(mode)?;
    }
    Ok(())
}

fn rejected_semantic_metadata_refunds(mode: MalformedSemanticMode) -> TestResult {
    let root = tempfile::tempdir()?;
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))?;
    let socket = root.path().join("native.sock");
    let calls = Arc::new(AtomicUsize::new(0));
    let (service, semantic) = service_with_options(
        root.path(),
        &socket,
        Arc::clone(&calls),
        ServiceOptions {
            malformed_semantic: Some(mode),
            ..ServiceOptions::default()
        },
    )?;
    let observation = service.resource_observation();
    let handle = service.effort_handle();
    let running = RunningService::start(service);
    let client = NativeUdsClient::new(socket);
    let (view, _) = setup(&client, semantic)?;
    let catalog_path = catalog_path(root.path());
    let initial_pins = pin_count(&catalog_path)?;
    let provider_before = calls.load(Ordering::SeqCst);
    let expected_provider_calls = provider_before
        .checked_add(1)
        .ok_or("call count overflow")?;
    let policy = policy()?;
    let estimator = DeclaredCostEstimator;
    let signals = UnknownSignals;
    let mut work = WorkBudget::with_request(limits(), budget());
    assert!(matches!(
        handle.query_effort(
            request(view, EffortPreference::Deep),
            NativeEffortControl {
                policy: &policy,
                estimator: &estimator,
                signals: &signals,
                clock: None,
                history: None,
            },
            &mut work,
        ),
        Err(IndexError::BackendViolation(_))
    ));
    assert_eq!(calls.load(Ordering::SeqCst), expected_provider_calls);
    assert_eq!(pin_count(&catalog_path)?, initial_pins);
    let resources = observation.observe()?;
    assert_eq!(resources.reserved, ResourceNeed::default());
    assert_eq!(resources.interactive.active, 0);
    assert_eq!(resources.enrichment.active, 0);
    running.stop()?;
    Ok(())
}

#[test]
fn managed_native_service_excludes_standalone_object_namespace_openers() -> TestResult {
    let root = tempfile::tempdir()?;
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))?;
    let socket = root.path().join("native.sock");
    let namespace = root.path().join("native-objects");
    let (service, _) = service(root.path(), &socket, Arc::new(AtomicUsize::new(0)))?;
    let handle = service.effort_handle();
    let running = RunningService::start(service);
    assert!(matches!(
        FileObjectStore::open(&namespace),
        Err(IndexError::ResourceLimited)
    ));
    running.stop()?;
    assert!(matches!(
        FileObjectStore::open(&namespace),
        Err(IndexError::ResourceLimited)
    ));
    drop(handle);
    let reopened = FileObjectStore::open(&namespace)?;
    drop(reopened);
    Ok(())
}

#[test]
fn cancelling_original_request_after_fast_prevents_optional_provider() -> TestResult {
    let root = tempfile::tempdir()?;
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))?;
    let socket = root.path().join("native.sock");
    let calls = Arc::new(AtomicUsize::new(0));
    let (service, semantic) = service(root.path(), &socket, Arc::clone(&calls))?;
    let handle = service.effort_handle();
    let running = RunningService::start(service);
    let client = NativeUdsClient::new(socket);
    let (view, _) = setup(&client, semantic)?;
    let baseline = calls.load(Ordering::SeqCst);
    assert_eq!(baseline, 1);
    let request_budget = budget();
    let signals = CancelAfterFast {
        calls: AtomicUsize::new(0),
        cancel: request_budget.cancel_handle(),
    };
    let policy = policy()?;
    let estimator = DeclaredCostEstimator;
    let control = NativeEffortControl {
        policy: &policy,
        estimator: &estimator,
        signals: &signals,
        clock: None,
        history: None,
    };
    let mut work = WorkBudget::with_request(limits(), request_budget);
    assert!(matches!(
        handle.query_effort(request(view, EffortPreference::Deep), control, &mut work),
        Err(IndexError::Cancelled)
    ));
    assert!(signals.calls.load(Ordering::SeqCst) >= 2);
    assert_eq!(calls.load(Ordering::SeqCst), baseline);
    running.stop()?;
    Ok(())
}

#[test]
fn observer_receives_fast_before_deep_on_one_pinned_old_view() -> TestResult {
    let root = tempfile::tempdir()?;
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))?;
    let socket = root.path().join("native.sock");
    let calls = Arc::new(AtomicUsize::new(0));
    let (service, semantic) = service(root.path(), &socket, Arc::clone(&calls))?;
    let handle = service.effort_handle();
    let running = RunningService::start(service);
    let client = NativeUdsClient::new(socket);
    let (view_a, a_record) = setup(&client, semantic)?;
    let catalog_path = catalog_path(root.path());
    let original_pins = pin_count(&catalog_path)?;
    let expected_pins = original_pins.checked_add(1).ok_or("pin count overflow")?;
    let provider_before = calls.load(Ordering::SeqCst);
    let expected_provider_calls = provider_before
        .checked_add(1)
        .ok_or("call count overflow")?;
    let policy = policy()?;
    let estimator = DeclaredCostEstimator;
    let signals = UnknownSignals;
    let mut seen = Vec::new();
    let mut published_b = None;
    let result = {
        let mut observer = StageObserver(|stage: NativeEffortStage<'_>| {
            match stage {
                NativeEffortStage::Fast(fast) => {
                    if fast.view != view_a || pin_count(&catalog_path)? != expected_pins {
                        return Err(IndexError::BackendViolation(
                            "Fast callback lost its selected pin".into(),
                        ));
                    }
                    if calls.load(Ordering::SeqCst) != provider_before {
                        return Err(IndexError::BackendViolation(
                            "Deep provider ran before Fast callback".into(),
                        ));
                    }
                    seen.push((1_u8, fast.view));
                    let (view_b, _) = advance(&client, view_a, 4, 2, b"beta", Some(a_record))
                        .map_err(|error| IndexError::BackendViolation(error.to_string()))?;
                    published_b = Some(view_b);
                }
                NativeEffortStage::Deep(deep) => {
                    if seen.as_slice() != [(1_u8, view_a)]
                        || deep.view != view_a
                        || pin_count(&catalog_path)? != expected_pins
                    {
                        return Err(IndexError::BackendViolation(
                            "Deep callback changed stage order or selected pin".into(),
                        ));
                    }
                    seen.push((2_u8, deep.view));
                }
                NativeEffortStage::OverlayFast(_) | NativeEffortStage::OverlayDeep(_) => {
                    return Err(IndexError::BackendViolation(
                        "durable observer received an overlay stage".into(),
                    ));
                }
            }
            Ok(())
        });
        let mut work = WorkBudget::with_request(limits(), budget());
        handle.query_effort_with_observer(
            request(view_a, EffortPreference::Deep),
            NativeEffortControl {
                policy: &policy,
                estimator: &estimator,
                signals: &signals,
                clock: None,
                history: None,
            },
            &mut work,
            &mut observer,
        )
    }?;
    assert_eq!(seen.as_slice(), &[(1_u8, view_a), (2_u8, view_a)]);
    assert_ne!(
        published_b.ok_or("Fast callback did not publish B")?,
        view_a
    );
    assert_eq!(result.view, view_a);
    assert_eq!(result.fast.durable()?.view, view_a);
    assert_eq!(
        result.deep.ok_or("Deep result absent")?.durable()?.view,
        view_a
    );
    assert_eq!(calls.load(Ordering::SeqCst), expected_provider_calls);
    assert_eq!(pin_count(&catalog_path)?, original_pins);
    running.stop()?;
    Ok(())
}

#[test]
fn observer_cancellation_after_fast_blocks_deep_and_releases_pin() -> TestResult {
    let root = tempfile::tempdir()?;
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))?;
    let socket = root.path().join("native.sock");
    let calls = Arc::new(AtomicUsize::new(0));
    let (service, semantic) = service(root.path(), &socket, Arc::clone(&calls))?;
    let resources = service.resource_observation();
    let handle = service.effort_handle();
    let running = RunningService::start(service);
    let client = NativeUdsClient::new(socket);
    let (view, _) = setup(&client, semantic)?;
    let catalog_path = catalog_path(root.path());
    let original_pins = pin_count(&catalog_path)?;
    let expected_pins = original_pins.checked_add(1).ok_or("pin count overflow")?;
    let provider_before = calls.load(Ordering::SeqCst);
    let request_budget = budget();
    let cancel = request_budget.cancel_handle();
    let mut work = WorkBudget::with_request(limits(), request_budget);
    let policy = policy()?;
    let estimator = DeclaredCostEstimator;
    let signals = UnknownSignals;
    let mut fast_seen = false;
    let result = {
        let mut observer = StageObserver(|stage: NativeEffortStage<'_>| match stage {
            NativeEffortStage::Fast(fast) => {
                if fast.view != view || pin_count(&catalog_path)? != expected_pins {
                    return Err(IndexError::BackendViolation("Fast pin absent".into()));
                }
                let observed = resources.observe()?;
                if observed.reserved != ResourceNeed::default()
                    || observed.retained_memory_bytes == 0
                    || observed.live_memory_bytes != observed.retained_memory_bytes
                    || observed.pending_escape_bytes != 0
                {
                    return Err(IndexError::BackendViolation(
                        "profile memory must survive Fast without an execution slot".into(),
                    ));
                }
                fast_seen = true;
                cancel.cancel();
                Ok(())
            }
            NativeEffortStage::Deep(_)
            | NativeEffortStage::OverlayFast(_)
            | NativeEffortStage::OverlayDeep(_) => Err(IndexError::BackendViolation(
                "Deep callback ran after cancellation".into(),
            )),
        });
        handle.query_effort_with_observer(
            request(view, EffortPreference::Deep),
            NativeEffortControl {
                policy: &policy,
                estimator: &estimator,
                signals: &signals,
                clock: None,
                history: None,
            },
            &mut work,
            &mut observer,
        )
    };
    assert!(fast_seen);
    assert_eq!(result.err(), Some(IndexError::Cancelled));
    assert_eq!(calls.load(Ordering::SeqCst), provider_before);
    assert_eq!(pin_count(&catalog_path)?, original_pins);
    let after = resources.observe()?;
    assert_eq!(after.reserved, ResourceNeed::default());
    assert_eq!(after.live_memory_bytes, 0);
    assert_eq!(after.retained_memory_bytes, 0);
    assert_eq!(after.pending_escape_bytes, 0);
    running.stop()?;
    Ok(())
}

#[test]
fn observer_failure_after_fast_blocks_deep_and_releases_pin() -> TestResult {
    let root = tempfile::tempdir()?;
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))?;
    let socket = root.path().join("native.sock");
    let calls = Arc::new(AtomicUsize::new(0));
    let (service, semantic) = service(root.path(), &socket, Arc::clone(&calls))?;
    let resources = service.resource_observation();
    let handle = service.effort_handle();
    let running = RunningService::start(service);
    let client = NativeUdsClient::new(socket);
    let (view, _) = setup(&client, semantic)?;
    let catalog_path = catalog_path(root.path());
    let original_pins = pin_count(&catalog_path)?;
    let expected_pins = original_pins.checked_add(1).ok_or("pin count overflow")?;
    let provider_before = calls.load(Ordering::SeqCst);
    let policy = policy()?;
    let estimator = DeclaredCostEstimator;
    let signals = UnknownSignals;
    let mut observer = StageObserver(|stage: NativeEffortStage<'_>| match stage {
        NativeEffortStage::Fast(fast) if fast.view == view => {
            if pin_count(&catalog_path)? != expected_pins {
                return Err(IndexError::BackendViolation("Fast pin absent".into()));
            }
            Err(IndexError::Storage("observer write failed".into()))
        }
        NativeEffortStage::Fast(_)
        | NativeEffortStage::Deep(_)
        | NativeEffortStage::OverlayFast(_)
        | NativeEffortStage::OverlayDeep(_) => Err(IndexError::BackendViolation(
            "invalid observer stage".into(),
        )),
    });
    let mut work = WorkBudget::with_request(limits(), budget());
    let result = handle.query_effort_with_observer(
        request(view, EffortPreference::Deep),
        NativeEffortControl {
            policy: &policy,
            estimator: &estimator,
            signals: &signals,
            clock: None,
            history: None,
        },
        &mut work,
        &mut observer,
    );
    assert_eq!(
        result.err(),
        Some(IndexError::Storage("observer write failed".into()))
    );
    assert_eq!(calls.load(Ordering::SeqCst), provider_before);
    assert_eq!(pin_count(&catalog_path)?, original_pins);
    assert_eq!(resources.observe()?.reserved, ResourceNeed::default());
    running.stop()?;
    Ok(())
}

#[test]
fn text_result_last_shared_owner_releases_memory_after_execution() -> TestResult {
    let root = tempfile::tempdir()?;
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))?;
    let socket = root.path().join("native.sock");
    let (service, semantic) = service(root.path(), &socket, Arc::new(AtomicUsize::new(0)))?;
    let resources = service.resource_observation();
    let handle = service.effort_handle();
    let running = RunningService::start(service);
    let client = NativeUdsClient::new(socket);
    let (view, _) = setup(&client, semantic)?;
    let policy = policy()?;
    let estimator = DeclaredCostEstimator;
    let signals = UnknownSignals;
    let mut work = WorkBudget::with_request(limits(), budget());
    let result = handle.query_effort(
        request(view, EffortPreference::Fast),
        NativeEffortControl {
            policy: &policy,
            estimator: &estimator,
            signals: &signals,
            clock: None,
            history: None,
        },
        &mut work,
    )?;
    let observed = resources.observe()?;
    let payload_bytes =
        result
            .fast
            .durable()?
            .hits
            .iter()
            .try_fold(0_u64, |total, hit| -> TestResult<u64> {
                total
                    .checked_add(u64::try_from(hit.path.capacity())?)
                    .ok_or_else(|| "path bytes overflow".into())
            })?;
    assert!(observed.live_memory_bytes > payload_bytes);
    assert_eq!(observed.live_memory_bytes, observed.retained_memory_bytes);
    assert_eq!(observed.reserved, ResourceNeed::default());
    assert_eq!(observed.pending_escape_bytes, 0);
    let shared = result.fast.durable()?.clone();
    assert_eq!(shared.hits.as_ptr(), result.fast.durable()?.hits.as_ptr());
    assert_eq!(
        shared
            .hits
            .first()
            .ok_or("shared hit absent")?
            .path
            .as_ptr(),
        result
            .fast
            .durable()?
            .hits
            .first()
            .ok_or("original hit absent")?
            .path
            .as_ptr()
    );
    assert_eq!(
        resources.observe()?.live_memory_bytes,
        observed.live_memory_bytes
    );
    drop(result);
    assert_eq!(
        resources.observe()?.live_memory_bytes,
        observed.live_memory_bytes
    );
    assert_eq!(shared.view, view);
    drop(shared);
    let released = resources.observe()?;
    assert_eq!(released.live_memory_bytes, 0);
    assert_eq!(released.retained_memory_bytes, 0);
    assert_eq!(released.pending_escape_bytes, 0);
    running.stop()?;
    Ok(())
}

#[test]
fn cancelled_fast_observer_clone_keeps_data_without_any_execution_slot() -> TestResult {
    let root = tempfile::tempdir()?;
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))?;
    let socket = root.path().join("native.sock");
    let calls = Arc::new(AtomicUsize::new(0));
    let (service, semantic) = service(root.path(), &socket, Arc::clone(&calls))?;
    let resources = service.resource_observation();
    let handle = service.effort_handle();
    let running = RunningService::start(service);
    let client = NativeUdsClient::new(socket);
    let (view, _) = setup(&client, semantic)?;
    let catalog_path = catalog_path(root.path());
    let original_pins = pin_count(&catalog_path)?;
    let provider_before = calls.load(Ordering::SeqCst);
    let request_budget = budget();
    let cancel = request_budget.cancel_handle();
    let policy = policy()?;
    let estimator = DeclaredCostEstimator;
    let signals = UnknownSignals;
    let mut held = None;
    let mut work = WorkBudget::with_request(limits(), request_budget);
    let outcome = {
        let mut observer = StageObserver(|stage: NativeEffortStage<'_>| match stage {
            NativeEffortStage::Fast(fast) => {
                held = Some(fast.clone());
                cancel.cancel();
                Ok(())
            }
            NativeEffortStage::Deep(_)
            | NativeEffortStage::OverlayFast(_)
            | NativeEffortStage::OverlayDeep(_) => Err(IndexError::BackendViolation(
                "Deep ran after cancellation".into(),
            )),
        });
        handle.query_effort_with_observer(
            request(view, EffortPreference::Deep),
            NativeEffortControl {
                policy: &policy,
                estimator: &estimator,
                signals: &signals,
                clock: None,
                history: None,
            },
            &mut work,
            &mut observer,
        )
    };
    assert_eq!(outcome.err(), Some(IndexError::Cancelled));
    assert_eq!(calls.load(Ordering::SeqCst), provider_before);
    assert_eq!(pin_count(&catalog_path)?, original_pins);
    let observed = resources.observe()?;
    assert!(observed.live_memory_bytes > 0);
    assert_eq!(observed.live_memory_bytes, observed.retained_memory_bytes);
    assert_eq!(observed.reserved, ResourceNeed::default());
    assert_eq!(observed.pending_escape_bytes, 0);
    let held = held.ok_or("Fast observer clone missing")?;
    assert_eq!(held.view, view);
    assert_eq!(held.hits.first().ok_or("held hit absent")?.path, "a.rs");
    drop(held);
    assert_eq!(resources.observe()?.live_memory_bytes, 0);
    assert_eq!(resources.observe()?.retained_memory_bytes, 0);
    running.stop()?;
    Ok(())
}

#[test]
fn semantic_result_last_shared_owner_releases_memory_without_execution_slots() -> TestResult {
    let root = tempfile::tempdir()?;
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))?;
    let socket = root.path().join("native.sock");
    let (service, semantic) = service(root.path(), &socket, Arc::new(AtomicUsize::new(0)))?;
    let resources = service.resource_observation();
    let handle = service.effort_handle();
    let running = RunningService::start(service);
    let client = NativeUdsClient::new(socket);
    let (view, _) = setup(&client, semantic)?;
    let policy = policy()?;
    let mut work = WorkBudget::with_request(limits(), budget());
    let result = handle.query_effort(
        request(view, EffortPreference::Deep),
        NativeEffortControl {
            policy: &policy,
            estimator: &DeclaredCostEstimator,
            signals: &UnknownSignals,
            clock: None,
            history: None,
        },
        &mut work,
    )?;
    let deep = result.deep.ok_or("Deep result absent")?.into_durable()?;
    drop(result.fast);
    let observed = resources.observe()?;
    let hit = deep.hits.first().ok_or("Semantic hit absent")?;
    assert!(observed.live_memory_bytes > u64::try_from(hit.vector_id.capacity())?);
    assert_eq!(observed.live_memory_bytes, observed.retained_memory_bytes);
    assert_eq!(observed.reserved, ResourceNeed::default());
    assert_eq!(observed.pending_escape_bytes, 0);
    let shared = deep.clone();
    assert_eq!(shared.hits.as_ptr(), deep.hits.as_ptr());
    assert_eq!(
        shared
            .hits
            .first()
            .ok_or("shared hit absent")?
            .vector_id
            .as_ptr(),
        hit.vector_id.as_ptr()
    );
    assert_eq!(
        resources.observe()?.live_memory_bytes,
        observed.live_memory_bytes
    );
    drop(deep);
    assert_eq!(shared.view, view);
    assert_eq!(
        resources.observe()?.live_memory_bytes,
        observed.live_memory_bytes
    );
    drop(shared);
    let released = resources.observe()?;
    assert_eq!(released.live_memory_bytes, 0);
    assert_eq!(released.retained_memory_bytes, 0);
    assert_eq!(released.pending_escape_bytes, 0);
    running.stop()?;
    Ok(())
}

#[test]
fn cancelled_deep_observer_clone_retains_only_data_until_last_release() -> TestResult {
    struct Capture {
        cancel: CancelHandleV1,
        held: Option<quanta_index_searchd_runtime::native::ScopedNativeSemanticResult>,
    }
    impl NativeEffortObserver for Capture {
        fn observe(&mut self, stage: NativeEffortStage<'_>) -> Result<(), IndexError> {
            if let NativeEffortStage::Deep(result) = stage {
                self.held = Some(result.clone());
                self.cancel.cancel();
            }
            Ok(())
        }
    }
    let root = tempfile::tempdir()?;
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))?;
    let socket = root.path().join("native.sock");
    let (service, semantic) = service(root.path(), &socket, Arc::new(AtomicUsize::new(0)))?;
    let resources = service.resource_observation();
    let handle = service.effort_handle();
    let running = RunningService::start(service);
    let client = NativeUdsClient::new(socket);
    let (view, _) = setup(&client, semantic)?;
    let policy = policy()?;
    let original = budget();
    let mut observer = Capture {
        cancel: original.cancel_handle(),
        held: None,
    };
    let mut work = WorkBudget::with_request(limits(), original);
    let result = handle.query_effort_with_observer(
        request(view, EffortPreference::Deep),
        NativeEffortControl {
            policy: &policy,
            estimator: &DeclaredCostEstimator,
            signals: &UnknownSignals,
            clock: None,
            history: None,
        },
        &mut work,
        &mut observer,
    );
    assert!(matches!(result, Err(IndexError::Cancelled)));
    let snapshot = resources.observe()?;
    assert!(snapshot.live_memory_bytes > 0);
    assert_eq!(snapshot.live_memory_bytes, snapshot.retained_memory_bytes);
    assert_eq!(snapshot.reserved, ResourceNeed::default());
    assert_eq!(snapshot.pending_escape_bytes, 0);
    assert_eq!(
        observer.held.as_ref().ok_or("captured Deep absent")?.view,
        view
    );
    assert_eq!(pin_count(&catalog_path(root.path()))?, 0);
    observer.held = None;
    assert_eq!(resources.observe()?.live_memory_bytes, 0);
    running.stop()?;
    Ok(())
}

#[test]
fn observer_failure_after_deep_releases_pin_and_both_permits() -> TestResult {
    let root = tempfile::tempdir()?;
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))?;
    let socket = root.path().join("native.sock");
    let calls = Arc::new(AtomicUsize::new(0));
    let (service, semantic) = service(root.path(), &socket, Arc::clone(&calls))?;
    let resources = service.resource_observation();
    let handle = service.effort_handle();
    let running = RunningService::start(service);
    let client = NativeUdsClient::new(socket);
    let (view, _) = setup(&client, semantic)?;
    let catalog_path = catalog_path(root.path());
    let original_pins = pin_count(&catalog_path)?;
    let expected_pins = original_pins.checked_add(1).ok_or("pin count overflow")?;
    let provider_before = calls.load(Ordering::SeqCst);
    let expected_provider_calls = provider_before
        .checked_add(1)
        .ok_or("call count overflow")?;
    let policy = policy()?;
    let estimator = DeclaredCostEstimator;
    let signals = UnknownSignals;
    let mut fast_seen = false;
    let mut deep_seen = false;
    let result = {
        let mut observer = StageObserver(|stage: NativeEffortStage<'_>| match stage {
            NativeEffortStage::Fast(fast) if fast.view == view => {
                if pin_count(&catalog_path)? != expected_pins {
                    return Err(IndexError::BackendViolation("Fast pin absent".into()));
                }
                fast_seen = true;
                Ok(())
            }
            NativeEffortStage::Deep(deep) if fast_seen && deep.view == view => {
                if pin_count(&catalog_path)? != expected_pins {
                    return Err(IndexError::BackendViolation("Deep pin absent".into()));
                }
                deep_seen = true;
                Err(IndexError::Storage("Deep observer write failed".into()))
            }
            NativeEffortStage::Fast(_)
            | NativeEffortStage::Deep(_)
            | NativeEffortStage::OverlayFast(_)
            | NativeEffortStage::OverlayDeep(_) => Err(IndexError::BackendViolation(
                "invalid observer stage order or view".into(),
            )),
        });
        let mut work = WorkBudget::with_request(limits(), budget());
        handle.query_effort_with_observer(
            request(view, EffortPreference::Deep),
            NativeEffortControl {
                policy: &policy,
                estimator: &estimator,
                signals: &signals,
                clock: None,
                history: None,
            },
            &mut work,
            &mut observer,
        )
    };
    assert!(fast_seen && deep_seen);
    assert_eq!(
        result.err(),
        Some(IndexError::Storage("Deep observer write failed".into()))
    );
    assert_eq!(calls.load(Ordering::SeqCst), expected_provider_calls);
    assert_eq!(pin_count(&catalog_path)?, original_pins);
    let observed = resources.observe()?;
    assert_eq!(observed.reserved, ResourceNeed::default());
    assert_eq!(observed.interactive.active, 0);
    assert_eq!(observed.enrichment.active, 0);
    running.stop()?;
    Ok(())
}

#[test]
fn effort_terminal_callback_runs_once_before_selected_pin_release() -> TestResult {
    let root = tempfile::tempdir()?;
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))?;
    let socket = root.path().join("native.sock");
    let calls = Arc::new(AtomicUsize::new(0));
    let (service, semantic) = service(root.path(), &socket, Arc::clone(&calls))?;
    let handle = service.effort_handle();
    let running = RunningService::start(service);
    let client = NativeUdsClient::new(socket);
    let (view, _) = setup(&client, semantic)?;
    let catalog_path = catalog_path(root.path());
    let original_pins = pin_count(&catalog_path)?;
    let expected_pins = original_pins.checked_add(1).ok_or("pin count overflow")?;
    let policy = policy()?;
    let estimator = DeclaredCostEstimator;
    let signals = UnknownSignals;
    let mut observer = TerminalCustodyObserver {
        catalog: &catalog_path,
        expected_pins,
        view,
        stages: Vec::new(),
        terminal: None,
    };
    let mut work = WorkBudget::with_request(limits(), budget());
    let result = handle.query_effort_with_observer(
        request(view, EffortPreference::Deep),
        NativeEffortControl {
            policy: &policy,
            estimator: &estimator,
            signals: &signals,
            clock: None,
            history: None,
        },
        &mut work,
        &mut observer,
    )?;
    let summary = observer.terminal.ok_or("missing terminal callback")?;
    assert_eq!(summary.view, result.view);
    assert_eq!(summary.reason, result.reason);
    assert_eq!(summary.deep_delivered, result.deep.is_some());
    assert_eq!(observer.stages, vec![1, 2]);
    assert_eq!(pin_count(&catalog_path)?, original_pins);
    running.stop()?;
    Ok(())
}

#[test]
fn uds_stream_delivers_fast_while_deep_provider_is_still_blocked() -> TestResult {
    let root = tempfile::tempdir()?;
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))?;
    let socket = root.path().join("native.sock");
    let calls = Arc::new(AtomicUsize::new(0));
    let gate = Arc::new(ProviderGate::default());
    let (service, semantic) = service_with_options(
        root.path(),
        &socket,
        Arc::clone(&calls),
        ServiceOptions {
            provider_gate: Some(Arc::clone(&gate)),
            ..ServiceOptions::default()
        },
    )?;
    let resources = service.resource_observation();
    service.configure_effort(NativeEffortConfiguration {
        history: Arc::new(NativeEffortObservationHistory::new()),
        policy: Arc::new(policy()?),
        estimator: Arc::new(DeclaredCostEstimator),
        signals: Arc::new(UnknownSignals),
    })?;
    let running = RunningService::start(service);
    let client = NativeUdsClient::new(socket.clone());
    let (view, _) = setup(&client, semantic)?;
    let catalog_path = catalog_path(root.path());
    let baseline_pins = pin_count(&catalog_path)?;
    let (stage_tx, stage_rx) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        let client = NativeUdsClient::new(socket);
        let mut stages = Vec::new();
        let result = client.query_effort_stream(stream_query(view), &budget(), &mut |stage| {
            let observed = match stage {
                NativeEffortStreamStage::Fast(result) => (1_u8, result.view),
                NativeEffortStreamStage::Deep(result) => (2_u8, result.view),
                NativeEffortStreamStage::OverlayFast(_)
                | NativeEffortStreamStage::OverlayDeep(_) => {
                    return Err(NativeUdsError::Protocol(
                        "durable effort returned an overlay stage".into(),
                    ));
                }
            };
            stages.push(observed);
            stage_tx
                .send(observed)
                .map_err(|error| NativeUdsError::Protocol(error.to_string()))?;
            Ok(())
        });
        (result, stages)
    });
    let first = stage_rx.recv_timeout(Duration::from_secs(3))?;
    assert_eq!(first, (1, view));
    wait_until(|| gate.entered.load(Ordering::Acquire))?;
    assert!(matches!(
        stage_rx.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));
    assert_eq!(pin_count(&catalog_path)?, baseline_pins + 1);
    gate.released.store(true, Ordering::Release);
    let (result, stages) = worker.join().map_err(|_panic| "stream worker panicked")?;
    let done = result?;
    assert_eq!(stages, vec![(1, view), (2, view)]);
    assert_eq!(done.view, view);
    assert!(done.deep_delivered);
    // The client may decode the terminal before the server finishes the
    // post-write ticket release; check bounded eventual cleanup.
    wait_until(|| {
        pin_count(&catalog_path).is_ok_and(|count| count == baseline_pins)
            && resources.observe().is_ok_and(|observation| {
                observation.reserved == ResourceNeed::default()
                    && observation.interactive.active == 0
                    && observation.enrichment.active == 0
            })
    })?;
    let final_resources = resources.observe()?;
    assert_eq!(final_resources.reserved, ResourceNeed::default());
    assert_eq!(final_resources.interactive.active, 0);
    assert_eq!(final_resources.enrichment.active, 0);
    running.stop()?;
    Ok(())
}

#[test]
fn uds_stream_client_abort_after_fast_releases_pin_and_reservations() -> TestResult {
    let root = tempfile::tempdir()?;
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))?;
    let socket = root.path().join("native.sock");
    let calls = Arc::new(AtomicUsize::new(0));
    let gate = Arc::new(ProviderGate::default());
    let (service, semantic) = service_with_options(
        root.path(),
        &socket,
        Arc::clone(&calls),
        ServiceOptions {
            provider_gate: Some(Arc::clone(&gate)),
            ..ServiceOptions::default()
        },
    )?;
    let resources = service.resource_observation();
    service.configure_effort(NativeEffortConfiguration {
        history: Arc::new(NativeEffortObservationHistory::new()),
        policy: Arc::new(policy()?),
        estimator: Arc::new(DeclaredCostEstimator),
        signals: Arc::new(UnknownSignals),
    })?;
    let running = RunningService::start(service);
    let client = NativeUdsClient::new(socket.clone());
    let (view, _) = setup(&client, semantic)?;
    let catalog_path = catalog_path(root.path());
    let baseline_pins = pin_count(&catalog_path)?;
    let (fast_tx, fast_rx) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        let client = NativeUdsClient::new(socket);
        client.query_effort_stream(stream_query(view), &budget(), &mut |stage| {
            if matches!(stage, NativeEffortStreamStage::Fast(_)) {
                fast_tx
                    .send(())
                    .map_err(|error| NativeUdsError::Protocol(error.to_string()))?;
                return Err(NativeUdsError::Protocol("abort after Fast".into()));
            }
            Err(NativeUdsError::Protocol("Deep escaped abort".into()))
        })
    });
    fast_rx.recv_timeout(Duration::from_secs(3))?;
    assert!(matches!(
        worker.join().map_err(|_panic| "stream worker panicked")?,
        Err(NativeUdsError::Protocol(_))
    ));
    gate.released.store(true, Ordering::Release);
    wait_until(|| {
        pin_count(&catalog_path).is_ok_and(|count| count == baseline_pins)
            && resources.observe().is_ok_and(|observation| {
                observation.reserved == ResourceNeed::default()
                    && observation.interactive.active == 0
                    && observation.enrichment.active == 0
            })
    })?;
    running.stop()?;
    Ok(())
}

fn configured_auto_stream(
    signals: Arc<dyn NativeEffortSignalPort>,
    expected_reason: quanta_index_contract::native_protocol::NativeEffortReason,
    expected_deep: bool,
) -> TestResult {
    let root = tempfile::tempdir()?;
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))?;
    let socket = root.path().join("native.sock");
    let calls = Arc::new(AtomicUsize::new(0));
    let (service, semantic) = service(root.path(), &socket, Arc::clone(&calls))?;
    let resources = service.resource_observation();
    service.configure_effort(NativeEffortConfiguration {
        history: Arc::new(NativeEffortObservationHistory::new()),
        policy: Arc::new(policy()?),
        estimator: Arc::new(DeclaredCostEstimator),
        signals,
    })?;
    let running = RunningService::start(service);
    let client = NativeUdsClient::new(socket);
    let (view, _) = setup(&client, semantic)?;
    let catalog_path = catalog_path(root.path());
    let pins_before = pin_count(&catalog_path)?;
    let calls_before = calls.load(Ordering::SeqCst);
    let mut query = stream_query(view);
    query.preference = NativeEffortPreference::Auto;
    let mut stages = Vec::new();
    let done = client.query_effort_stream(query, &budget(), &mut |stage| {
        let entry = match stage {
            NativeEffortStreamStage::Fast(result) => (1_u8, result.view),
            NativeEffortStreamStage::Deep(result) => (2_u8, result.view),
            NativeEffortStreamStage::OverlayFast(_) | NativeEffortStreamStage::OverlayDeep(_) => {
                return Err(NativeUdsError::Protocol(
                    "durable effort returned an overlay stage".into(),
                ));
            }
        };
        stages.push(entry);
        Ok(())
    })?;
    assert_eq!(done.view, view);
    assert_eq!(done.reason, expected_reason);
    assert_eq!(done.deep_delivered, expected_deep);
    assert_eq!(
        stages,
        if expected_deep {
            vec![(1, view), (2, view)]
        } else {
            vec![(1, view)]
        }
    );
    let expected_calls = calls_before
        .checked_add(usize::from(expected_deep))
        .ok_or("provider call count overflow")?;
    assert_eq!(calls.load(Ordering::SeqCst), expected_calls);
    wait_until(|| {
        pin_count(&catalog_path).is_ok_and(|pins| pins == pins_before)
            && resources
                .observe()
                .is_ok_and(|state| state.reserved == ResourceNeed::default())
    })?;
    running.stop()?;
    Ok(())
}

#[test]
fn uds_auto_unknown_signals_finish_fast_without_provider_dispatch() -> TestResult {
    configured_auto_stream(
        Arc::new(UnknownSignals),
        quanta_index_contract::native_protocol::NativeEffortReason::UnknownTelemetry,
        false,
    )
}

#[test]
fn uds_auto_pressure_signals_finish_fast_without_provider_dispatch() -> TestResult {
    configured_auto_stream(
        Arc::new(HighPressureSignals),
        quanta_index_contract::native_protocol::NativeEffortReason::Pressure,
        false,
    )
}

struct DwelledClearSignals(AtomicUsize);
impl NativeEffortSignalPort for DwelledClearSignals {
    fn sample(&self, now_ms: u64) -> EffortSignals {
        // Let the first clear sample commit, then advance real time during
        // the second observation. Time spent inside the first sample alone
        // must not count toward the clear dwell window.
        if self.0.fetch_add(1, Ordering::SeqCst) == 1 {
            std::thread::sleep(Duration::from_millis(3));
        }
        ClearSignals.sample(now_ms)
    }
}

#[test]
fn uds_auto_clear_dwell_delivers_deep_on_the_original_view() -> TestResult {
    configured_auto_stream(
        Arc::new(DwelledClearSignals(AtomicUsize::new(0))),
        quanta_index_contract::native_protocol::NativeEffortReason::StableRefinement,
        true,
    )
}

#[test]
fn uds_auto_without_configuration_refuses_before_provider_dispatch() -> TestResult {
    let root = tempfile::tempdir()?;
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))?;
    let socket = root.path().join("native.sock");
    let calls = Arc::new(AtomicUsize::new(0));
    let (service, semantic) = service(root.path(), &socket, Arc::clone(&calls))?;
    let running = RunningService::start(service);
    let client = NativeUdsClient::new(socket);
    let (view, _) = setup(&client, semantic)?;
    let before = calls.load(Ordering::SeqCst);
    let mut query = stream_query(view);
    query.preference = NativeEffortPreference::Auto;
    let mut stage_count = 0_usize;
    let result = client.query_effort_stream(query, &budget(), &mut |_stage| {
        stage_count = stage_count
            .checked_add(1)
            .ok_or_else(|| NativeUdsError::Protocol("stage count overflow".into()))?;
        Ok(())
    });
    assert!(
        matches!(result, Err(NativeUdsError::Refused(error)) if error.code == quanta_index_contract::native_protocol::NativeIndexErrorCode::CapabilityUnavailable)
    );
    assert_eq!(stage_count, 0);
    assert_eq!(calls.load(Ordering::SeqCst), before);
    running.stop()?;
    Ok(())
}

struct CancelOnFinishObserver<'a> {
    inner: TerminalCustodyObserver<'a>,
    cancel: CancelHandleV1,
}

impl NativeEffortObserver for CancelOnFinishObserver<'_> {
    fn observe(&mut self, stage: NativeEffortStage<'_>) -> Result<(), IndexError> {
        self.inner.observe(stage)
    }

    fn finish(&mut self, summary: NativeEffortSummary<'_>) -> Result<(), IndexError> {
        self.inner.finish(summary)?;
        self.cancel.cancel();
        Ok(())
    }
}

fn cancellation_during_terminal_callback(preference: EffortPreference) -> TestResult {
    let root = tempfile::tempdir()?;
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))?;
    let socket = root.path().join("native.sock");
    let calls = Arc::new(AtomicUsize::new(0));
    let (service, semantic) = service(root.path(), &socket, Arc::clone(&calls))?;
    let resources = service.resource_observation();
    let handle = service.effort_handle();
    let running = RunningService::start(service);
    let client = NativeUdsClient::new(socket);
    let (view, _) = setup(&client, semantic)?;
    let catalog_path = catalog_path(root.path());
    let original_pins = pin_count(&catalog_path)?;
    let expected_pins = original_pins.checked_add(1).ok_or("pin count overflow")?;
    let policy = policy()?;
    let estimator = DeclaredCostEstimator;
    let signals = UnknownSignals;
    let original = budget();
    let mut observer = CancelOnFinishObserver {
        inner: TerminalCustodyObserver {
            catalog: &catalog_path,
            expected_pins,
            view,
            stages: Vec::new(),
            terminal: None,
        },
        cancel: original.cancel_handle(),
    };
    let mut work = WorkBudget::with_request(limits(), original.clone());
    let result = handle.query_effort_with_observer(
        request(view, preference),
        NativeEffortControl {
            policy: &policy,
            estimator: &estimator,
            signals: &signals,
            clock: None,
            history: None,
        },
        &mut work,
        &mut observer,
    );
    assert!(original.is_cancelled());
    let terminal = observer.inner.terminal.ok_or("missing terminal callback")?;
    assert_eq!(terminal.view, view);
    assert_eq!(
        terminal.deep_delivered,
        preference == EffortPreference::Deep
    );
    assert_eq!(pin_count(&catalog_path)?, original_pins);
    assert_eq!(resources.observe()?.reserved, ResourceNeed::default());
    running.stop()?;
    assert!(
        matches!(result, Err(IndexError::Cancelled)),
        "terminal callback cancellation became success"
    );
    Ok(())
}

#[test]
fn fast_terminal_callback_cancellation_refuses_success_and_releases_custody() -> TestResult {
    cancellation_during_terminal_callback(EffortPreference::Fast)
}

#[test]
fn deep_terminal_callback_cancellation_refuses_success_and_releases_custody() -> TestResult {
    cancellation_during_terminal_callback(EffortPreference::Deep)
}
