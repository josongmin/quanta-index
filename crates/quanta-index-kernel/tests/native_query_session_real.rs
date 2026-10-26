//! Real text and semantic adapters must share a selected native query session.
#![expect(
    clippy::disallowed_methods,
    reason = "test-only synchronous driver executes Kernel futures"
)]
#![expect(
    clippy::panic_in_result_fn,
    reason = "integration assertions are independent behavior oracles"
)]

use std::{
    error::Error,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
    time::Duration,
};

use futures::executor::block_on;
use quanta_index_catalog::{CatalogClockPort, SqliteCatalog, SystemCatalogClock};
use quanta_index_contract::{
    EmbeddingNormalization, LqExpr, LqLeaf, LqQuery, LqSpan, QueryConstraintSetV1,
    SemanticQueryRequest,
};
use quanta_index_core::index::{
    CollectionId, CoverageBinding, CoverageStatus, Digest, GroupActivation, IndexError,
    IndexInstanceId, IndexKind, IndexProfile, IndexQueryPort, InputDomain, InstanceSpec, ObjectRef,
    OperationKey, OperationStatus, OwnerKey, OwnerMutation, ProducerId, PublicationGroupId,
    QueryCoverage, QueryOutcome, QueryReadContext, SchemaId, SlotId, SourceCut, SourceEvent,
    SourceTextAdmission, SourceTransition, StableOwnerId, StorageFormatId, StreamId, SubmitOutcome,
    TenantId, UpdateKind, UpdateRequest, ViewRef, ViewSelection, WorkBudget, WorkLimits,
    WorkspaceKey, WorktreeId,
    effort::{ReservationGuard, ResourceNeed, ResourceScheduler},
};
use quanta_index_core::{CoreError, RequestBudgetV1, TextEmbeddingProvider};
use quanta_index_kernel::{
    AdapterDescriptor, AdapterRegistry, Kernel, KernelServices, QueryAdapterRegistry,
};
use quanta_index_lexical::{
    native::{
        Bm25Policy, NativeTextIndex, NativeTextReader, PlannedTextRead, TextReadResult, TextRecipe,
    },
    planner::LexicalPlanner,
};
use quanta_index_semantic::native::{
    NativeSemantic, NativeSemanticReadConfig, NativeSemanticReader, SearchResult,
    SemanticReadRequest, TextEmbeddingBridge,
};
use quanta_index_storage::{FileObjectStore, FileScratchSpace};

type TestResult<T = ()> = Result<T, Box<dyn Error>>;

#[path = "native_query_session_real/ann_regions.rs"]
mod ann_regions;
const PRODUCER: ProducerId = ProducerId([51; 16]);
const TEXT_INSTANCE: IndexInstanceId = IndexInstanceId([52; 16]);
const SEMANTIC_INSTANCE: IndexInstanceId = IndexInstanceId([53; 16]);
const ADMISSION: Digest = Digest([54; 32]);

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

fn owner() -> OwnerKey {
    OwnerKey {
        producer: PRODUCER,
        stable_id: StableOwnerId([6; 16]),
    }
}

fn text_spec(recipe: TextRecipe) -> InstanceSpec {
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

fn budget() -> WorkBudget {
    WorkBudget::with_request(
        limits(),
        RequestBudgetV1::for_duration(Duration::from_secs(30)),
    )
}

fn published(outcome: SubmitOutcome) -> TestResult<ViewRef> {
    match outcome {
        SubmitOutcome::Accepted(attempt) => {
            Err(format!("publication still accepted: {attempt:?}").into())
        }
        SubmitOutcome::Terminal(status) => match status {
            OperationStatus::Published(view) => Ok(view),
            OperationStatus::Unknown => Err("publication status unknown".into()),
            OperationStatus::Pending(attempt) => {
                Err(format!("publication still pending: {attempt:?}").into())
            }
            OperationStatus::Refused(reason) => {
                Err(format!("publication refused: {reason:?}").into())
            }
        },
        SubmitOutcome::Unknown(operation) => {
            Err(format!("publication outcome unknown: {operation:?}").into())
        }
    }
}

struct FixedProvider {
    calls: Arc<AtomicUsize>,
}

impl TextEmbeddingProvider for FixedProvider {
    fn embed_batch(
        &self,
        texts: &[&str],
    ) -> Result<quanta_index_core::TextEmbeddingBatch, CoreError> {
        texts
            .iter()
            .map(|text| match *text {
                "alpha" | "query" => Ok(vec![1.0, 0.0]),
                "beta" => Ok(vec![0.0, 1.0]),
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
        request.checkpoint("session:embed")?;
        let _previous = self.calls.fetch_add(1, Ordering::SeqCst);
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
        "session-fixed-model"
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

struct Fixture {
    kernel: Kernel<SqliteCatalog>,
    catalog: Arc<SqliteCatalog>,
    text: QueryAdapterRegistry<PlannedTextRead, TextReadResult>,
    semantic: QueryAdapterRegistry<SemanticReadRequest, SearchResult>,
    recipe: quanta_index_semantic::native::EmbeddingRecipe,
    calls: Arc<AtomicUsize>,
}

fn fixture(root: &Path) -> TestResult<Fixture> {
    fixture_with_clock(root, Arc::new(SystemCatalogClock))
}

fn fixture_with_clock(root: &Path, clock: Arc<dyn CatalogClockPort>) -> TestResult<Fixture> {
    fixture_with_options(root, clock, None)
}

fn fixture_with_ann(root: &Path, runtime: Arc<tokio::runtime::Runtime>) -> TestResult<Fixture> {
    fixture_with_options(root, Arc::new(SystemCatalogClock), Some(runtime))
}

fn fixture_with_options(
    root: &Path,
    clock: Arc<dyn CatalogClockPort>,
    ann_runtime: Option<Arc<tokio::runtime::Runtime>>,
) -> TestResult<Fixture> {
    let ann_enabled = ann_runtime.is_some();
    let read_lease = if ann_enabled {
        Duration::from_secs(120)
    } else {
        Duration::from_millis(100)
    };
    let scratch = Arc::new(FileScratchSpace::open_standalone(
        root.join("scratch"),
        16 * 1024 * 1024 * 1024,
    )?);
    let objects = Arc::new(FileObjectStore::open_with_scratch(
        root.join("objects"),
        scratch.clone(),
    )?);
    let catalog = Arc::new(SqliteCatalog::open_with_clock(root, read_lease, clock)?);
    let calls = Arc::new(AtomicUsize::new(0));
    let bridge = Arc::new(TextEmbeddingBridge::new(FixedProvider {
        calls: Arc::clone(&calls),
    })?);
    let recipe = bridge.recipe(1024)?;
    let text_recipe = TextRecipe::new(ADMISSION);
    let text_spec = text_spec(text_recipe);
    let semantic_spec = semantic_spec(&recipe);
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
        NativeTextIndex::new(text_recipe),
        |delta| Ok(delta.clone()),
    )?;
    let semantic_builder = NativeSemantic::new(Arc::clone(&bridge), recipe.clone())?;
    let semantic_builder = if ann_enabled {
        semantic_builder.with_ann()
    } else {
        semantic_builder
    };
    builders.register_owner(semantic_descriptor, semantic_builder)?;
    let kernel = Kernel::new(
        KernelServices::new(
            objects.clone(),
            objects,
            catalog.clone(),
            scratch,
            if ann_enabled {
                read_lease
            } else {
                Duration::from_secs(30)
            },
        )?,
        builders,
    );
    let text_need = ResourceNeed {
        cpu_units: 1,
        io_bytes: 64 * 1024,
        memory_bytes: 1024 * 1024,
        provider_units: 0,
    };
    let semantic_need = ResourceNeed {
        provider_units: 1,
        ..text_need
    };
    let mut text = QueryAdapterRegistry::new();
    text.register(
        text_descriptor,
        text_need,
        NativeTextReader::new(text_recipe, Bm25Policy { k1: 1.2, b: 0.75 }, text_need)?,
    )?;
    let mut semantic = QueryAdapterRegistry::new();
    let semantic_reader = NativeSemanticReader::new(
        bridge,
        recipe.clone(),
        NativeSemanticReadConfig {
            page_size: 8,
            max_owners: if ann_enabled { 80 } else { 8 },
            max_query_bytes: 1024,
            need: semantic_need,
        },
    )?;
    let semantic_reader = if let Some(runtime) = ann_runtime {
        semantic_reader.with_ann(runtime)
    } else {
        semantic_reader
    };
    semantic.register(semantic_descriptor, semantic_need, semantic_reader)?;
    Ok(Fixture {
        kernel,
        catalog,
        text,
        semantic,
        recipe,
        calls,
    })
}

fn source(kernel: &Kernel<SqliteCatalog>, view: ViewRef) -> TestResult<SourceCut> {
    Ok(kernel
        .pin_view(ViewSelection::Exact(view), &budget())?
        .view()
        .source)
}

fn configured(fixture: &Fixture) -> TestResult<ViewRef> {
    let genesis = fixture
        .kernel
        .create_workspace(workspace(), 1, &mut budget())?;
    let ingress = fixture.kernel.begin_input(operation(1))?;
    let profile = fixture.kernel.stage_profile(
        ingress,
        &IndexProfile {
            instances: vec![
                text_spec(TextRecipe::new(ADMISSION)),
                semantic_spec(&fixture.recipe),
            ],
        },
        &mut budget(),
    )?;
    published(block_on(fixture.kernel.submit(
        UpdateRequest {
            operation: operation(1),
            expected_view: genesis,
            kind: UpdateKind::Reconfigure { profile },
        },
        &mut budget(),
    ))?)
}

fn advance(
    fixture: &Fixture,
    view: ViewRef,
    sequence: u64,
    event: u64,
    body: &[u8],
    previous: Option<ObjectRef>,
) -> TestResult<(ViewRef, ObjectRef)> {
    let operation_key = operation(sequence);
    let ingress = fixture.kernel.begin_input(operation_key)?;
    let record = fixture.kernel.stage_source_text(
        ingress,
        "a.rs",
        body,
        SourceTextAdmission::Utf8 { policy: ADMISSION },
        &mut budget(),
    )?;
    let owners = fixture.kernel.stage_mutations(
        ingress,
        std::iter::once(Ok(OwnerMutation {
            owner: owner(),
            expected: previous,
            next: Some(record),
        })),
        &mut budget(),
    )?;
    let mut transition = SourceTransition {
        expected_source: source(&fixture.kernel, view)?,
        owners,
        events: Vec::new(),
    };
    transition.events.push(SourceEvent {
        producer: PRODUCER,
        stream: StreamId([5; 16]),
        stream_epoch: 1,
        predecessor: event
            .checked_sub(1)
            .ok_or("event sequence must start at one")?,
        sequence: event,
        payload: transition.owner_digest()?,
    });
    let next = published(block_on(fixture.kernel.submit(
        UpdateRequest {
            operation: operation_key,
            expected_view: view,
            kind: UpdateKind::SourceAdvance(transition),
        },
        &mut budget(),
    ))?)?;
    Ok((next, record))
}

fn materialize(fixture: &Fixture, view: ViewRef, sequence: u64) -> TestResult<ViewRef> {
    let ready = fixture
        .kernel
        .ready_work(workspace(), 1, 8, &mut budget())?;
    assert_eq!(ready.len(), 1);
    let [work] = ready.as_slice() else {
        return Err("expected one semantic owner work item".into());
    };
    let claim = fixture
        .kernel
        .claim_work(work, Duration::from_secs(30), &budget())?;
    published(block_on(fixture.kernel.submit(
        UpdateRequest {
            operation: operation(sequence),
            expected_view: view,
            kind: UpdateKind::Materialize { claim },
        },
        &mut budget(),
    ))?)
}

fn text_request(term: &str) -> TestResult<PlannedTextRead> {
    let mut query = LqQuery::empty(LqSpan::synthetic(0));
    query.expr = LqExpr::Leaf(LqLeaf::Keyword(term.into()));
    Ok(PlannedTextRead::new(
        LexicalPlanner::plan(&query)?,
        1,
        &quanta_index_contract::QueryConstraintSetV1::unconstrained(),
        |bytes, lifetime| budget().reserve_memory(bytes, lifetime),
    )?)
}

fn repeated_text_request(terms: &[&str]) -> TestResult<PlannedTextRead> {
    let mut query = LqQuery::empty(LqSpan::synthetic(0));
    query.expr = LqExpr::All(
        terms
            .iter()
            .map(|term| LqExpr::Leaf(LqLeaf::Keyword((*term).into())))
            .collect(),
    );
    Ok(PlannedTextRead::new(
        LexicalPlanner::plan(&query)?,
        1,
        &quanta_index_contract::QueryConstraintSetV1::unconstrained(),
        |bytes, lifetime| budget().reserve_memory(bytes, lifetime),
    )?)
}

fn semantic_request() -> SemanticReadRequest {
    SemanticQueryRequest {
        query_text: "query".into(),
        constraints: QueryConstraintSetV1::unconstrained(),
        generation: None,
        generation_selector: None,
        lexical_scope: None,
        top_k: 1,
    }
    .into()
}

struct TestScheduler;
impl ResourceScheduler for TestScheduler {
    type Token = ();
    fn try_reserve(&self, _need: ResourceNeed) -> Option<Self::Token> {
        Some(())
    }
    fn release(&self, _token: Self::Token) {}
}

fn guard(need: ResourceNeed) -> TestResult<ReservationGuard<TestScheduler>> {
    ReservationGuard::try_acquire(Arc::new(TestScheduler), need)
        .ok_or_else(|| "test scheduler refused reservation".into())
}

fn native_pin_count(catalog: &SqliteCatalog) -> TestResult<i64> {
    Ok(rusqlite::Connection::open(catalog.path())?.query_row(
        "SELECT COUNT(*) FROM native_pin_v1",
        [],
        |row| row.get(0),
    )?)
}

fn native_pin_lease(catalog: &SqliteCatalog) -> TestResult<(i64, i64, i64)> {
    Ok(rusqlite::Connection::open(catalog.path())?.query_row(
        "SELECT ticket,fence,expires_at_ms FROM native_pin_v1",
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?)
}

struct ManualClock(AtomicU64);

impl CatalogClockPort for ManualClock {
    fn now_unix_ms(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}

struct WrongStoredSemanticReader {
    selected: InstanceSpec,
    binding: CoverageBinding,
    need: ResourceNeed,
    calls: Arc<AtomicUsize>,
}

impl IndexQueryPort<SemanticReadRequest, SearchResult> for WrongStoredSemanticReader {
    fn validate_instance(&self, instance: &InstanceSpec) -> Result<(), IndexError> {
        if *instance != self.selected {
            return Err(IndexError::InvalidInput(
                "unexpected semantic instance".into(),
            ));
        }
        Ok(())
    }

    fn reservation(&self, _request: &SemanticReadRequest) -> Result<ResourceNeed, IndexError> {
        Ok(self.need)
    }

    fn query(
        &self,
        _request: &SemanticReadRequest,
        context: &mut QueryReadContext<'_>,
    ) -> Result<QueryOutcome<SearchResult>, IndexError> {
        let _previous = self.calls.fetch_add(1, Ordering::SeqCst);
        QueryOutcome::async_ranked(
            context.ticket,
            context.instance,
            &self.binding,
            false,
            SearchResult::from_parts(
                Vec::new(),
                0,
                0,
                quanta_index_contract::native_protocol::NativeSemanticTraversal::ExactCapped,
                false,
                None,
            ),
        )
    }
}

#[test]
fn duplicate_folded_terms_share_reads_and_keep_the_pinned_view_after_delta() -> TestResult {
    let root = tempfile::tempdir()?;
    let fixture = fixture(root.path())?;
    let configured = configured(&fixture)?;
    let (view_a, record_a) = advance(&fixture, configured, 2, 1, b"alpha", None)?;

    let mut single_budget = budget();
    let single = fixture.kernel.prepare_query(
        &fixture.text,
        ViewSelection::Exact(view_a),
        TEXT_INSTANCE,
        text_request("alpha")?,
        &mut single_budget,
    )?;
    let need = single.required_resources();
    let single = single.execute(&guard(need)?)?;
    assert_eq!(single.result().matched, 1);
    assert_eq!(single.result().hits.len(), 1);

    let aliases: Vec<&str> = ["alpha", "ALPHA"].into_iter().cycle().take(64).collect();
    let mut repeated_budget = budget();
    let repeated = fixture.kernel.prepare_query(
        &fixture.text,
        ViewSelection::Exact(view_a),
        TEXT_INSTANCE,
        repeated_text_request(&aliases)?,
        &mut repeated_budget,
    )?;
    let need = repeated.required_resources();
    let repeated = repeated.execute(&guard(need)?)?;
    assert_eq!(repeated.result(), single.result());
    // Parsing/normalizing extra leaves may charge bytes, but must not reopen
    // the immutable member, root or posting for the same canonical term.
    assert_eq!(repeated_budget.usage().reads, single_budget.usage().reads);

    let mut pinned_budget = budget();
    let mut pinned = fixture
        .kernel
        .begin_query_session(ViewSelection::Exact(view_a), &mut pinned_budget)?;
    let (view_b, record_b) = advance(&fixture, view_a, 3, 2, b"beta", Some(record_a))?;
    assert_ne!(view_a, view_b);
    assert_ne!(record_a, record_b);
    let retained = pinned.prepare_stage(
        &fixture.text,
        TEXT_INSTANCE,
        repeated_text_request(&aliases)?,
    )?;
    let need = retained.required_resources();
    let retained = retained.execute(&guard(need)?)?;
    assert_eq!(retained.view(), view_a);
    assert_eq!(retained.result(), single.result());
    pinned.close()?;

    let mut latest_budget = budget();
    let latest = fixture.kernel.prepare_query(
        &fixture.text,
        ViewSelection::Exact(view_b),
        TEXT_INSTANCE,
        repeated_text_request(&aliases)?,
        &mut latest_budget,
    )?;
    let need = latest.required_resources();
    let latest = latest.execute(&guard(need)?)?;
    assert_eq!(latest.result().matched, 0);
    assert!(latest.result().hits.is_empty());
    assert_eq!(fixture.calls.load(Ordering::SeqCst), 0);
    assert_eq!(native_pin_count(&fixture.catalog)?, 0);
    Ok(())
}

#[test]
fn duplicate_terms_still_enforce_the_original_leaf_cap_and_release_the_pin() -> TestResult {
    let root = tempfile::tempdir()?;
    let fixture = fixture(root.path())?;
    let configured = configured(&fixture)?;
    let (view, _) = advance(&fixture, configured, 2, 1, b"alpha", None)?;
    let aliases = vec!["alpha"; 65];
    let mut work = budget();
    let query = fixture.kernel.prepare_query(
        &fixture.text,
        ViewSelection::Exact(view),
        TEXT_INSTANCE,
        repeated_text_request(&aliases)?,
        &mut work,
    )?;
    let need = query.required_resources();
    assert!(matches!(
        query.execute(&guard(need)?),
        Err(IndexError::InvalidInput(_))
    ));
    assert_eq!(native_pin_count(&fixture.catalog)?, 0);
    Ok(())
}

#[test]
fn real_text_then_semantic_and_text_keep_view_a_after_body_b_publishes() -> TestResult {
    let root = tempfile::tempdir()?;
    let fixture = fixture(root.path())?;
    let configured = configured(&fixture)?;
    let (first, a_record) = advance(&fixture, configured, 2, 1, b"alpha", None)?;
    let view_a = materialize(&fixture, first, 3)?;
    let old_source = source(&fixture.kernel, view_a)?;
    assert_eq!(fixture.calls.load(Ordering::SeqCst), 1);
    let mut work = budget();
    let mut session = fixture
        .kernel
        .begin_query_session(ViewSelection::Exact(view_a), &mut work)?;
    assert_eq!(native_pin_count(&fixture.catalog)?, 1);

    let fast = session.prepare_stage(&fixture.text, TEXT_INSTANCE, text_request("alpha")?)?;
    let fast_need = fast.required_resources();
    let fast = fast.execute(&guard(fast_need)?)?;
    assert_eq!(fast.view(), view_a);
    let [fast_hit] = fast.result().hits.as_slice() else {
        return Err("expected exactly one text hit".into());
    };
    assert_eq!(
        fast_hit.source,
        quanta_index_lexical::native::NativeTextHitSource::Durable(a_record)
    );
    assert_eq!(fast_hit.path, "a.rs");
    assert!(matches!(fast.coverage(), QueryCoverage::Core));

    let (view_b, b_record) = advance(&fixture, view_a, 4, 2, b"beta", Some(a_record))?;
    assert_ne!(a_record, b_record);
    assert_ne!(source(&fixture.kernel, view_b)?.owners, old_source.owners);
    assert_eq!(native_pin_count(&fixture.catalog)?, 1);
    let deep = session.prepare_stage(&fixture.semantic, SEMANTIC_INSTANCE, semantic_request())?;
    let deep_need = deep.required_resources();
    let deep = deep.execute(&guard(deep_need)?)?;
    assert_eq!(deep.view(), view_a);
    assert!(deep.exhaustive());
    let [deep_hit] = deep.result().hits.as_slice() else {
        return Err("expected exactly one semantic hit".into());
    };
    assert_eq!(deep_hit.owner, owner());
    assert_eq!(deep_hit.cosine.to_bits(), 1.0f64.to_bits());
    match deep.coverage() {
        QueryCoverage::Async(binding) => {
            assert_eq!(binding.status, CoverageStatus::Complete);
            assert_eq!(binding.desired.root, old_source.owners);
            assert_eq!(binding.covered, Some(binding.desired));
        }
        QueryCoverage::Core => return Err("semantic stage returned core coverage".into()),
    }

    let again = session.prepare_stage(&fixture.text, TEXT_INSTANCE, text_request("alpha")?)?;
    let again_need = again.required_resources();
    let again = again.execute(&guard(again_need)?)?;
    assert_eq!(again.view(), view_a);
    let [again_hit] = again.result().hits.as_slice() else {
        return Err("expected exactly one retained-view text hit".into());
    };
    assert_eq!(
        again_hit.source,
        quanta_index_lexical::native::NativeTextHitSource::Durable(a_record)
    );
    assert_eq!(fixture.calls.load(Ordering::SeqCst), 2);
    assert_eq!(native_pin_count(&fixture.catalog)?, 1);
    session.close()?;
    assert_eq!(native_pin_count(&fixture.catalog)?, 0);
    Ok(())
}

#[test]
fn cancellation_of_original_request_after_fast_prevents_semantic_provider() -> TestResult {
    let root = tempfile::tempdir()?;
    let fixture = fixture(root.path())?;
    let configured = configured(&fixture)?;
    let (first, _) = advance(&fixture, configured, 2, 1, b"alpha", None)?;
    let view = materialize(&fixture, first, 3)?;
    let baseline_calls = fixture.calls.load(Ordering::SeqCst);
    assert_eq!(baseline_calls, 1);
    let request = RequestBudgetV1::for_duration(Duration::from_secs(30));
    let cancel = request.cancel_handle();
    let mut work = WorkBudget::with_request(limits(), request);
    let mut session = fixture
        .kernel
        .begin_query_session(ViewSelection::Exact(view), &mut work)?;
    let fast = session.prepare_stage(&fixture.text, TEXT_INSTANCE, text_request("alpha")?)?;
    let fast_need = fast.required_resources();
    assert_eq!(fast.execute(&guard(fast_need)?)?.result().hits.len(), 1);
    cancel.cancel();
    let deep = session.prepare_stage(&fixture.semantic, SEMANTIC_INSTANCE, semantic_request());
    match deep {
        Ok(stage) => {
            let need = stage.required_resources();
            assert!(matches!(
                stage.execute(&guard(need)?),
                Err(IndexError::Cancelled)
            ));
        }
        Err(error) => assert_eq!(error, IndexError::Cancelled),
    }
    assert_eq!(fixture.calls.load(Ordering::SeqCst), baseline_calls);
    assert_eq!(native_pin_count(&fixture.catalog)?, 1);
    session.close()?;
    assert_eq!(native_pin_count(&fixture.catalog)?, 0);
    assert!(
        work.usage().reads > 0,
        "stages must consume the original budget"
    );
    Ok(())
}

#[test]
fn wrong_stored_async_coverage_is_refused_then_real_semantic_stage_uses_same_pin() -> TestResult {
    let root = tempfile::tempdir()?;
    let fixture = fixture(root.path())?;
    let configured = configured(&fixture)?;
    let (first, _) = advance(&fixture, configured, 2, 1, b"alpha", None)?;
    let view = materialize(&fixture, first, 3)?;
    let mut work = budget();
    let mut session = fixture
        .kernel
        .begin_query_session(ViewSelection::Exact(view), &mut work)?;
    let original_pin = native_pin_lease(&fixture.catalog)?;

    // Obtain the selected stored binding through the real reader, then forge
    // only its context. The resulting Complete binding still passes the
    // public QueryOutcome shape checks.
    let first = session.prepare_stage(&fixture.semantic, SEMANTIC_INSTANCE, semantic_request())?;
    let semantic_need = first.required_resources();
    let first = first.execute(&guard(semantic_need)?)?;
    let selected = match first.coverage() {
        QueryCoverage::Async(binding) => **binding,
        QueryCoverage::Core => return Err("semantic stage returned core coverage".into()),
    };
    assert_eq!(selected.status, CoverageStatus::Complete);
    assert_eq!(selected.covered, Some(selected.desired));
    let mut forged = selected;
    forged.desired.context = Digest([99; 32]);
    forged.covered = Some(forged.desired);
    assert_ne!(forged, selected);

    let selected_spec = semantic_spec(&fixture.recipe);
    let mut wrong_readers = QueryAdapterRegistry::new();
    let wrong_calls = Arc::new(AtomicUsize::new(0));
    wrong_readers.register(
        AdapterDescriptor {
            kind: selected_spec.kind,
            schema: selected_spec.schema,
            format: selected_spec.format,
        },
        semantic_need,
        WrongStoredSemanticReader {
            selected: selected_spec,
            binding: forged,
            need: semantic_need,
            calls: Arc::clone(&wrong_calls),
        },
    )?;
    let wrong = session.prepare_stage(&wrong_readers, SEMANTIC_INSTANCE, semantic_request())?;
    let wrong_need = wrong.required_resources();
    assert!(matches!(
        wrong.execute(&guard(wrong_need)?),
        Err(IndexError::BackendViolation(_))
    ));
    assert_eq!(wrong_calls.load(Ordering::SeqCst), 1);
    assert_eq!(native_pin_count(&fixture.catalog)?, 1);
    assert_eq!(native_pin_lease(&fixture.catalog)?.0, original_pin.0);

    let good = session.prepare_stage(&fixture.semantic, SEMANTIC_INSTANCE, semantic_request())?;
    let good_need = good.required_resources();
    let good = good.execute(&guard(good_need)?)?;
    assert_eq!(good.view(), view);
    assert_eq!(good.result().hits.len(), 1);
    assert!(matches!(good.coverage(), QueryCoverage::Async(binding) if **binding == selected));
    assert_eq!(native_pin_count(&fixture.catalog)?, 1);
    assert_eq!(native_pin_lease(&fixture.catalog)?.0, original_pin.0);
    session.close()?;
    assert_eq!(native_pin_count(&fixture.catalog)?, 0);
    Ok(())
}

#[test]
fn explicit_renew_extends_same_real_ticket_with_new_fence() -> TestResult {
    let root = tempfile::tempdir()?;
    let clock = Arc::new(ManualClock(AtomicU64::new(1_000)));
    let fixture = fixture_with_clock(root.path(), clock.clone())?;
    let configured = configured(&fixture)?;
    let (first, _) = advance(&fixture, configured, 2, 1, b"alpha", None)?;
    let view = materialize(&fixture, first, 3)?;
    let mut work = budget();
    let mut session = fixture
        .kernel
        .begin_query_session(ViewSelection::Exact(view), &mut work)?;
    let before = native_pin_lease(&fixture.catalog)?;
    assert_eq!(native_pin_count(&fixture.catalog)?, 1);

    clock.0.store(2_000, Ordering::SeqCst);
    session.renew()?;
    let after = native_pin_lease(&fixture.catalog)?;
    assert_eq!(after.0, before.0, "renewal must retain the ticket ID");
    assert_ne!(after.1, before.1, "renewal must fence the old lease");
    assert!(after.2 > before.2, "renewal must extend expiry");
    assert_eq!(native_pin_count(&fixture.catalog)?, 1);

    let deep = session.prepare_stage(&fixture.semantic, SEMANTIC_INSTANCE, semantic_request())?;
    let deep_need = deep.required_resources();
    let deep = deep.execute(&guard(deep_need)?)?;
    assert_eq!(deep.view(), view);
    assert_eq!(deep.result().hits.len(), 1);
    assert_eq!(native_pin_lease(&fixture.catalog)?.0, before.0);
    session.close()?;
    assert_eq!(native_pin_count(&fixture.catalog)?, 0);
    Ok(())
}
