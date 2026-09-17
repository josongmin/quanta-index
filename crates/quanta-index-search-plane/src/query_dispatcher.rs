//! Search-plane query orchestration using the in-memory readiness ledger as the
//! source of truth.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::{Arc, Mutex, RwLock};

use crate::{
    ActivationCatalog, ActiveGenerationRecord, Ledger, SEARCH_OWNED_SEMANTIC_DIMENSION,
    SnapshotAcquireOutcome, SnapshotKey, SnapshotRegistries, SnapshotRegistryPolicy,
    lower_lexical_text_query,
    lowering::lower_sourcegraph_structural_query_text,
    query_embedder::{HashingQueryTextEmbedder, QueryTextEmbedderPort},
    readiness::{
        DocFacetState, HistoryAuthorityState, RuntimeMetadataState, StructuralAuthorityState,
    },
};
use quanta_index_contract::lex::{CommitSha, LexicalErrorCode};
use quanta_index_contract::{
    CandidateCountV1, CandidatePresenceV1, ChunkId, ChunkRecord,
    ClusterMembershipBatchReadRequestV1, ClusterMembershipBatchReadResponseV1, CommitCandidate,
    DiffCandidate, EarlyStopReason, EngineTouched, ExplanationRow, GenerationPin,
    GenerationSelector, HistoryCursor, HistoryQueryRequest, HybridQueryRequest,
    HybridQueryResponse, HybridSeedQueryRequest, HybridSeedQueryResponse, LQ_VERSION_TAG,
    LexicalCandidate, LqCase, LqExpr, LqFileScope, LqFilter, LqLeaf, LqOptions, LqPatternType,
    LqQuery, LqStructuralBlock, LqStructuralConstraint, LqStructuralConstraintOperand,
    LqStructuralExpr, LqStructuralHoleRef, LqStructuralNode, LqType, LqYesNoOnly,
    ManifestGeneration, OwnerDocKind, PlannerStage, PlannerTraceEntry,
    QueryConstraintIntersectionV1, QueryConstraintSetV1, QueryErrorRepair, QueryResultWindowV1,
    RepairClass, RepoId, RepoMapQueryRequest, RepoMapQueryResponse, RevisionId,
    RuntimeMetadataQueryRequest, SearchExplanation, SearchPlaneExplainQueryRequest,
    SearchPlaneExplainQueryResponse, SearchPlaneHistoryQueryResponse, SearchPlaneIpcError,
    SearchPlaneQueryIpcRequest, SearchPlaneQueryIpcResponse,
    SearchPlaneRuntimeMetadataQueryResponse, SearchPlaneStructuralQueryResponse,
    SearchPlaneTrackKind, SeedCandidate, SeedContribution, SeedLane, SemanticQueryRequest,
    SemanticQueryResponse, StructuralQueryRequest, SymbolCandidate, SymbolQueryRequest,
    SymbolQueryResponse, TextQueryRequest, TextQueryResponse, TextQuerySyntax,
    continuation_fetch_size,
};
use quanta_index_core::domains::structural::{
    StructuralExecutableFilter, StructuralProducerPort,
    StructuralQueryRequest as DomainStructuralQueryRequest,
};
use quanta_index_core::{
    CoreError, ExplainQueryPort, HybridOrchestratorPolicy, HybridQueryPort,
    LexicalCandidateExplanationV1, LexicalIndexOpenPort, LexicalPolicy, LexicalQueryPort,
    LexicalScoreEngineV1, LexicalSearchPageV1, LexicalSearcher, REQUEST_CANCELLED_CODE,
    REQUEST_DEADLINE_EXCEEDED_CODE, RepoMapPolicy, RepoMapQueryPort, RequestBudgetV1,
    SemanticIndexOpenPort, SemanticPolicy, SemanticQueryPort, SemanticSearchHitV1,
    SemanticSearcher, StructuralMatchBinding, StructuralMatchCandidate, StructuralService,
    timeref::{parse_rev_at_time_spec, parse_search_timeref_ms},
    validate_query_top_k,
};
use quanta_index_lq_obs::{
    CardinalityGuard, Dimensions, MetricKind, MetricSample, OBS_OVERFLOW_LABEL, ObsError,
    validate_dimensions,
};
use quanta_index_lq_regex::RegexExecutor;

const ERR_INVALID: &str = "INVALID_REQUEST";
const ERR_NOT_READY: &str = "NOT_READY";
const ERR_NOT_FOUND: &str = "NOT_FOUND";
const ERR_NOT_IMPLEMENTED: &str = "NOT_IMPLEMENTED";
const ERR_INTERNAL: &str = "INTERNAL";
const ERR_HISTORY_PRODUCER_UNAVAILABLE: &str = "HISTORY_PRODUCER_UNAVAILABLE";
const ERR_HISTORY_GENERATION_NOT_READY: &str = "HISTORY_GENERATION_NOT_READY";
const ERR_HISTORY_SHARD_UNAVAILABLE: &str = "HISTORY_SHARD_UNAVAILABLE";
const ERR_HISTORY_INVALID_TIMEREF: &str = "HISTORY_INVALID_TIMEREF";
const ERR_RUNTIME_CATALOG_NOT_READY: &str = "RUNTIME_CATALOG_NOT_READY";
const ERR_RUNTIME_CATALOG_HEAD_MISSING: &str = "RUNTIME_CATALOG_HEAD_MISSING";
const ERR_RUNTIME_INVALID_SCOPE: &str = "RUNTIME_INVALID_SCOPE";
#[cfg(test)]
const ERR_RUNTIME_DIRTY_ONLY_UNSUPPORTED: &str = "RUNTIME_DIRTY_ONLY_UNSUPPORTED";
const ERR_SNAPSHOT_UNKNOWN: &str = "SNAPSHOT_UNKNOWN";

struct PreparedLanguageQueryV1 {
    query: LqQuery,
    constraints: QueryConstraintSetV1,
    force_empty: bool,
}

/// Compose DSL `lang:` filters with the typed OR-set once.
///
/// Remove the DSL language leaves so every sparse and dense lane consumes the
/// same canonical constraint. The two surfaces intersect; a disjoint
/// intersection is an explicit empty result, never an unconstrained fallback.
fn prepare_language_query_v1(
    mut query: LqQuery,
    typed: &QueryConstraintSetV1,
) -> Result<PreparedLanguageQueryV1, CoreError> {
    let mut dsl_languages = BTreeSet::new();
    let mut retained = Vec::with_capacity(query.filters.len());
    for filter in std::mem::take(&mut query.filters) {
        #[expect(
            clippy::wildcard_enum_match_arm,
            reason = "new non-language filters must remain executable; only lang filters are consumed into the typed constraint set"
        )]
        match filter {
            LqFilter::Lang { id } => {
                let canonical = id.trim().to_ascii_lowercase();
                let language =
                    quanta_index_contract::lex::LanguageCode::new(canonical).map_err(|err| {
                        CoreError::InvalidContract(format!(
                            "query language constraint is not canonical: {err}"
                        ))
                    })?;
                let _inserted = dsl_languages.insert(language);
            }
            other => retained.push(other),
        }
    }
    query.filters = retained;
    let dsl = QueryConstraintSetV1::from_languages(dsl_languages);
    let (constraints, force_empty) = match typed.intersect(&dsl) {
        QueryConstraintIntersectionV1::Compatible(constraints) => (constraints, false),
        QueryConstraintIntersectionV1::Contradiction => {
            (QueryConstraintSetV1::unconstrained(), true)
        }
    };
    Ok(PreparedLanguageQueryV1 {
        query,
        constraints,
        force_empty,
    })
}

/// Rows to fetch for one query so the window can observe a continuation row.
///
/// The public cap and the internal fetch ceiling are different numbers owned
/// by the contract: `top_k = 10_000` is accepted and fetches 10,001. This used
/// to refuse the public maximum because it compared `top_k + 1` against the
/// public cap itself (QI-BB-025).
fn probe_top_k_v1(top_k: u32) -> Result<u32, CoreError> {
    let accepted = validate_query_top_k(top_k)?;
    Ok(continuation_fetch_size(accepted))
}

/// Whether the query carries a `count` option, in which case the adapter
/// reports an exact total and the page needs no continuation probe.
fn requests_exact_total_v1(query: &LqQuery) -> bool {
    query.options.count.is_some()
}

/// Rows to ask the lexical adapter for.
///
/// The page plus one continuation probe, unless the adapter will report an
/// exact total anyway (QI-BB-005: `count:all` no longer widens the page; the
/// total comes from a count collector and the rows stay bounded by `top_k`).
fn lexical_fetch_limit_v1(query: &LqQuery, requested_top_k: u32) -> Result<u32, CoreError> {
    if requests_exact_total_v1(query) {
        return validate_query_top_k(requested_top_k);
    }
    probe_top_k_v1(requested_top_k)
}

/// Window for a page whose adapter proved the exact match total.
fn exact_total_window_v1(returned: usize, total: u64) -> Result<QueryResultWindowV1, CoreError> {
    let returned = u32::try_from(returned).map_err(|err| {
        CoreError::InvalidContract(format!("lexical page row count exceeds u32: {err}"))
    })?;
    if total < u64::from(returned) {
        return Err(CoreError::InvalidContract(format!(
            "lexical adapter reported an exact total of {total} below the {returned} rows it returned"
        )));
    }
    QueryResultWindowV1::new(
        returned,
        CandidateCountV1::Exact(total),
        total > u64::from(returned),
    )
    .map_err(|err| CoreError::InvalidContract(format!("lexical exact window: {err}")))
}

/// Window for one lexical page: exact when the adapter proved the total,
/// otherwise derived from the continuation probe.
///
/// `fetched_top_k` is what the adapter was asked for (the page, or the page
/// plus its probe row); more rows than that is a contract defect. With an
/// exact total the probe row, if any, is simply cut — the total already
/// says whether more exist.
fn lexical_page_window_v1(
    page: &mut LexicalSearchPageV1,
    requested_top_k: u32,
    fetched_top_k: u32,
) -> Result<QueryResultWindowV1, CoreError> {
    let fetched = top_k_limit(fetched_top_k);
    if page.candidates.len() > fetched {
        return Err(CoreError::InvalidContract(format!(
            "lexical adapter returned {} rows for a fetch of {fetched}",
            page.candidates.len()
        )));
    }
    match page.exact_total {
        Some(total) => {
            page.candidates.truncate(top_k_limit(requested_top_k));
            exact_total_window_v1(page.candidates.len(), total)
        }
        None => finalize_probe_window_v1(&mut page.candidates, requested_top_k),
    }
}

fn hybrid_probe_top_k_v1(top_k: u32) -> Result<u32, CoreError> {
    Ok(HybridOrchestratorPolicy::over_fetch_top_k(top_k).max(probe_top_k_v1(top_k)?))
}

fn finalize_probe_window_v1<T>(
    results: &mut Vec<T>,
    top_k: u32,
) -> Result<QueryResultWindowV1, CoreError> {
    let observed = results.len();
    let requested = usize::try_from(top_k)
        .map_err(|err| CoreError::InvalidContract(format!("query top_k overflow: {err}")))?;
    if results.len() > requested {
        results.truncate(requested);
    }
    QueryResultWindowV1::from_probe(top_k, observed)
        .map_err(|err| CoreError::InvalidContract(format!("query result window: {err}")))
}

fn fused_window_v1(
    top_k: u32,
    returned: usize,
    observed_universe: usize,
    lane_limit_reached: bool,
) -> Result<QueryResultWindowV1, CoreError> {
    let requested = usize::try_from(top_k)
        .map_err(|err| CoreError::InvalidContract(format!("query top_k overflow: {err}")))?;
    if lane_limit_reached && returned != requested {
        return Err(CoreError::InvalidContract(
            "hybrid result window observed a capped lane before filling the requested page"
                .to_string(),
        ));
    }
    let observed = if observed_universe > requested || lane_limit_reached {
        requested.saturating_add(1)
    } else {
        returned
    };
    QueryResultWindowV1::from_probe(top_k, observed)
        .map_err(|err| CoreError::InvalidContract(format!("query result window: {err}")))
}

pub trait QueryObsSink {
    fn emit(&self, sample: MetricSample);
}

struct NoopQueryObsSink;

impl QueryObsSink for NoopQueryObsSink {
    fn emit(&self, _sample: MetricSample) {}
}

const MAX_OBS_SAMPLES: usize = 4_096;

#[derive(Default)]
pub struct BoundedQueryObsStore {
    guard: Mutex<CardinalityGuard>,
    samples: Mutex<VecDeque<MetricSample>>,
    errors: Mutex<Vec<ObsError>>,
}

impl BoundedQueryObsStore {
    fn record_error(&self, err: ObsError) {
        let mut guard = lock_or_recover(&self.errors);
        guard.push(err);
    }

    #[must_use]
    pub fn snapshot(&self) -> Vec<MetricSample> {
        lock_or_recover(&self.samples).iter().cloned().collect()
    }

    #[must_use]
    pub fn errors(&self) -> Vec<ObsError> {
        lock_or_recover(&self.errors).clone()
    }
}

impl QueryObsSink for BoundedQueryObsStore {
    fn emit(&self, sample: MetricSample) {
        if let Err(err) = validate_dimensions(&sample.dimensions) {
            self.record_error(err);
            return;
        }
        let sample = {
            let mut guard = lock_or_recover(&self.guard);
            match guard.observe(&sample.dimensions) {
                Ok(()) => sample,
                Err(err) => {
                    self.record_error(err.clone());
                    overflow_bucket_sample(sample, &err)
                }
            }
        };
        let mut samples = lock_or_recover(&self.samples);
        if samples.len() == MAX_OBS_SAMPLES {
            let _evicted = samples.pop_front();
        }
        samples.push_back(sample);
    }
}

fn overflow_bucket_sample(mut sample: MetricSample, err: &ObsError) -> MetricSample {
    match err.dim_overflow.as_deref() {
        Some("tenant_id") => {
            sample.dimensions.tenant_id = OBS_OVERFLOW_LABEL.into();
        }
        Some("repo_id") => {
            sample.dimensions.repo_id = OBS_OVERFLOW_LABEL.into();
        }
        Some("ticket_id") => {
            sample.dimensions.ticket_id = OBS_OVERFLOW_LABEL.into();
        }
        Some("wave_id") => {
            sample.dimensions.wave_id = OBS_OVERFLOW_LABEL.into();
        }
        Some(_) | None => {}
    }
    sample
}

fn lock_or_recover<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(err) => err.into_inner(),
    }
}

fn classify_error_metric_name(err: &CoreError) -> &'static str {
    match err {
        CoreError::Typed { code, .. }
            if code == REQUEST_DEADLINE_EXCEEDED_CODE || code == REQUEST_CANCELLED_CODE =>
        {
            "lq_typed_error_interrupted_total"
        }
        CoreError::Typed { code, .. }
            if code.contains("PARSE")
                || code.contains("TRANSLATE_FAIL")
                || code.contains("INVALID_VECTOR")
                || code.contains("HOLE_KIND_UNSUPPORTED") =>
        {
            "lq_typed_error_parse_total"
        }
        CoreError::Typed { code, .. } if code.contains("DIRTY_ONLY_UNSUPPORTED") => {
            "lq_typed_error_invalid_request_total"
        }
        CoreError::Typed { code, .. }
            if code.contains("UNAVAILABLE")
                || code.contains("NOT_IMPLEMENTED")
                || code.contains("NOT_FOUND") =>
        {
            "lq_typed_error_unavailable_total"
        }
        CoreError::Typed { code, .. }
            if code.contains("PLAN_LIMIT")
                || code.contains("BUDGET_EXCEEDED")
                || code.contains("QUERY_TIMEOUT")
                || code.contains("COUNT_INVALID") =>
        {
            "lq_typed_error_plan_limit_total"
        }
        CoreError::NotReady(_) => "lq_typed_error_not_ready_total",
        CoreError::Typed { code, .. } if code.contains("NOT_READY") => {
            "lq_typed_error_not_ready_total"
        }
        CoreError::Storage(_) => "lq_typed_error_internal_total",
        CoreError::InvalidContract(_) => "lq_typed_error_invalid_request_total",
        CoreError::NotImplemented(_) | CoreError::NotFound(_) => "lq_typed_error_unavailable_total",
        CoreError::Typed { .. } => "lq_typed_error_other_total",
    }
}

pub struct SearchPlaneDispatcher {
    lex_opener: Arc<dyn LexicalIndexOpenPort + Send + Sync>,
    sem_opener: Arc<dyn SemanticIndexOpenPort + Send + Sync>,
    /// Resident opened generations, shared with the ingest side which
    /// invalidates them (QI-BB-001).
    snapshots: SnapshotRegistries,
    repo_map_query: Arc<dyn RepoMapQueryPort + Send + Sync>,
    /// Structural producer adapter wired by the composition root.
    structural_producer: Arc<dyn StructuralProducerPort + Send + Sync>,
    ledger: Arc<RwLock<Ledger>>,
    activation_catalog: Arc<ActivationCatalog>,
    query_embedder: Arc<dyn QueryTextEmbedderPort + Send + Sync>,
    obs_sink: Arc<dyn QueryObsSink + Send + Sync>,
}

pub type SearchPlaneQueryService = SearchPlaneDispatcher;
pub type SearchPlaneQueryDispatcher = SearchPlaneDispatcher;

mod semantic_query;
use semantic_query::{
    HybridFusion, SeedLaneTallyV1, SemanticScopeV1, SemanticSelection,
    build_hybrid_response_explanation, build_hybrid_seed_candidates,
    build_hybrid_seed_response_explanation, build_semantic_response_explanation,
    canonical_dense_corpus_budgets_v1, ensure_query_model_matches_index_v1,
    prefix_semantic_query_error, resolve_hybrid_request_selection,
    resolve_hybrid_seed_request_selection, resolve_semantic_request_selection,
};

impl SearchPlaneDispatcher {
    #[must_use]
    pub fn new(
        lex_opener: Arc<dyn LexicalIndexOpenPort + Send + Sync>,
        sem_opener: Arc<dyn SemanticIndexOpenPort + Send + Sync>,
        repo_map_query: Arc<dyn RepoMapQueryPort + Send + Sync>,
        structural_producer: Arc<dyn StructuralProducerPort + Send + Sync>,
        ledger: Arc<RwLock<Ledger>>,
        activation_catalog: Arc<ActivationCatalog>,
    ) -> Self {
        Self::new_with_obs(
            lex_opener,
            sem_opener,
            SnapshotRegistries::new(SnapshotRegistryPolicy::DEFAULT),
            repo_map_query,
            structural_producer,
            ledger,
            activation_catalog,
            Arc::new(HashingQueryTextEmbedder::new(
                SEARCH_OWNED_SEMANTIC_DIMENSION,
            )),
            Arc::new(NoopQueryObsSink),
        )
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "composition-root wiring of one collaborator per port; the registries and obs sink are shared with the ingest side and cannot be folded into an opener"
    )]
    pub fn new_with_obs(
        lex_opener: Arc<dyn LexicalIndexOpenPort + Send + Sync>,
        sem_opener: Arc<dyn SemanticIndexOpenPort + Send + Sync>,
        snapshots: SnapshotRegistries,
        repo_map_query: Arc<dyn RepoMapQueryPort + Send + Sync>,
        structural_producer: Arc<dyn StructuralProducerPort + Send + Sync>,
        ledger: Arc<RwLock<Ledger>>,
        activation_catalog: Arc<ActivationCatalog>,
        query_embedder: Arc<dyn QueryTextEmbedderPort + Send + Sync>,
        obs_sink: Arc<dyn QueryObsSink + Send + Sync>,
    ) -> Self {
        Self {
            lex_opener,
            sem_opener,
            snapshots,
            repo_map_query,
            structural_producer,
            ledger,
            activation_catalog,
            query_embedder,
            obs_sink,
        }
    }

    /// Acquire the shared lexical handle for a pinned sealed generation.
    ///
    /// Goes through the snapshot registry: a resident handle is returned
    /// without touching disk, a miss runs the adapter's cold open once and
    /// concurrent misses wait for it. The outcome is emitted as a metric
    /// under the pin's dimensions.
    fn acquire_lexical(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        generation: ManifestGeneration,
    ) -> Result<Arc<dyn LexicalSearcher>, CoreError> {
        let key = SnapshotKey::new(repo_id, revision_id, generation);
        let acquired = self.snapshots.lexical.acquire(&key, || {
            let handle: Arc<dyn LexicalSearcher> =
                Arc::from(self.lex_opener.open(repo_id, revision_id, generation)?);
            let resident_bytes = handle.resident_bytes_estimate();
            Ok(crate::OpenedSnapshot {
                handle,
                resident_bytes,
            })
        })?;
        self.emit_snapshot_metric("lexical", &key, acquired.outcome);
        Ok(acquired.handle)
    }

    /// Semantic counterpart of [`Self::acquire_lexical`].
    fn acquire_semantic(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        generation: ManifestGeneration,
    ) -> Result<Arc<dyn SemanticSearcher>, CoreError> {
        let key = SnapshotKey::new(repo_id, revision_id, generation);
        let acquired = self.snapshots.semantic.acquire(&key, || {
            let handle: Arc<dyn SemanticSearcher> =
                Arc::from(self.sem_opener.open(repo_id, revision_id, generation)?);
            let resident_bytes = handle.resident_bytes_estimate();
            Ok(crate::OpenedSnapshot {
                handle,
                resident_bytes,
            })
        })?;
        self.emit_snapshot_metric("semantic", &key, acquired.outcome);
        Ok(acquired.handle)
    }

    fn emit_snapshot_metric(
        &self,
        track: &'static str,
        key: &SnapshotKey,
        outcome: SnapshotAcquireOutcome,
    ) {
        let dimensions = Dimensions::new(
            "LXE-10",
            "8",
            "local",
            key.repo_id.as_str(),
            key.generation.get(),
        );
        let (name, kind, value) = match outcome {
            SnapshotAcquireOutcome::Hit => match track {
                "lexical" => ("lq_snapshot_lexical_hit_total", MetricKind::Counter, 1.0),
                _ => ("lq_snapshot_semantic_hit_total", MetricKind::Counter, 1.0),
            },
            SnapshotAcquireOutcome::Coalesced => match track {
                "lexical" => (
                    "lq_snapshot_lexical_coalesced_total",
                    MetricKind::Counter,
                    1.0,
                ),
                _ => (
                    "lq_snapshot_semantic_coalesced_total",
                    MetricKind::Counter,
                    1.0,
                ),
            },
            SnapshotAcquireOutcome::Miss { cold_open_nanos } => {
                let millis = u64::try_from(cold_open_nanos.div_euclid(1_000_000))
                    .map_or(f64::MAX, |value| {
                        u32::try_from(value).map_or(f64::MAX, f64::from)
                    });
                match track {
                    "lexical" => (
                        "lq_snapshot_lexical_cold_open_ms",
                        MetricKind::Histogram,
                        millis,
                    ),
                    _ => (
                        "lq_snapshot_semantic_cold_open_ms",
                        MetricKind::Histogram,
                        millis,
                    ),
                }
            }
        };
        self.obs_sink
            .emit(MetricSample::new(name, kind, value, dimensions));
    }

    /// Lower a text request into the one executable lexical plan: lowered
    /// query, composed constraints and the generation it runs against.
    ///
    /// The ranked search and the per-candidate explanation (QI-BB-022) both
    /// plan here, so an explanation scores a candidate through exactly the
    /// plan that ranked it.
    fn plan_lexical_text_query(
        &self,
        request: &TextQueryRequest,
    ) -> Result<PlannedLexicalTextQuery, CoreError> {
        let lowered = lower_lexical_text_query(request)?;
        let prepared_language = prepare_language_query_v1(lowered, &request.constraints)?;
        let base_pin = resolve_lexical_request_pin(
            self.activation_catalog.as_ref(),
            request,
            SearchPlaneTrackKind::Lexical,
            "lexical",
        )?;
        let prepared = prepare_lexical_text_query_for_execution(
            self.activation_catalog.as_ref(),
            self.ledger.as_ref(),
            &base_pin,
            prepared_language.query,
        )?;
        LexicalPolicy::validate_query_with_constraints(
            &prepared.query,
            &prepared_language.constraints,
        )?;
        Ok(PlannedLexicalTextQuery {
            pin: prepared.pin,
            query: prepared.query,
            constraints: prepared_language.constraints,
            force_empty: prepared.force_empty || prepared_language.force_empty,
        })
    }

    /// Lower the request and forward it to the live lexical searcher.
    fn lexical(
        &self,
        request: &TextQueryRequest,
        budget: &RequestBudgetV1,
    ) -> Result<TextQueryResponse, CoreError> {
        budget.checkpoint("lexical:entry")?;
        let _accepted_top_k = validate_query_top_k(request.top_k)?;
        let planned = self.plan_lexical_text_query(request)?;
        let wants_file_owner_projection = query_selects_file_owner_projection(&planned.query);
        if planned.force_empty {
            return Ok(TextQueryResponse {
                generation: planned.pin,
                results: Vec::new(),
                window: QueryResultWindowV1::exact(0),
                file_owner_rows: wants_file_owner_projection.then(Vec::new),
            });
        }
        let pin = planned.pin.clone();
        let materialized = self.snapshot_lex_materialized(&pin.repo_id, &pin.revision_id)?;
        LexicalPolicy::validate_query_against_readiness(pin.manifest_generation, materialized)?;
        let searcher =
            self.acquire_lexical(&pin.repo_id, &pin.revision_id, pin.manifest_generation)?;
        let fetch_top_k = lexical_fetch_limit_v1(&planned.query, request.top_k)?;
        budget.checkpoint("lexical:search")?;
        let mut page =
            searcher.search_constrained(&planned.query, &planned.constraints, fetch_top_k)?;
        budget.checkpoint("lexical:project")?;
        stabilize_ranked_candidates(&mut page.candidates);
        let window = lexical_page_window_v1(&mut page, request.top_k, fetch_top_k)?;
        let results = page.candidates;
        let file_owner_rows = if wants_file_owner_projection {
            Some(searcher.project_file_owners(&results)?)
        } else {
            None
        };
        Ok(TextQueryResponse {
            generation: planned.pin,
            results,
            window,
            file_owner_rows,
        })
    }

    fn symbol(
        &self,
        request: SymbolQueryRequest,
        budget: &RequestBudgetV1,
    ) -> Result<SymbolQueryResponse, CoreError> {
        budget.checkpoint("symbol:entry")?;
        let _accepted_top_k = validate_query_top_k(request.top_k)?;
        let pin = resolve_optional_selection(
            self.activation_catalog.as_ref(),
            request.generation.clone(),
            request.generation_selector.as_ref(),
            SearchPlaneTrackKind::Lexical,
            "symbol",
        )?
        .ok_or_else(|| {
            CoreError::InvalidContract("symbol: generation selector required".to_string())
        })?;
        let lexical_request = TextQueryRequest {
            syntax: request.syntax,
            query_text: request.query_text,
            constraints: request.constraints,
            generation: Some(pin.clone()),
            generation_selector: None,
            top_k: request.top_k,
        };
        let lowered = lower_lexical_text_query(&lexical_request)?;
        let prepared_language = prepare_language_query_v1(lowered, &lexical_request.constraints)?;
        LexicalPolicy::validate_query_with_constraints(
            &prepared_language.query,
            &prepared_language.constraints,
        )?;
        if prepared_language.force_empty {
            return Ok(SymbolQueryResponse {
                generation: pin,
                results: Vec::new(),
                window: QueryResultWindowV1::exact(0),
            });
        }
        let materialized = self.snapshot_lex_materialized(&pin.repo_id, &pin.revision_id)?;
        LexicalPolicy::validate_query_against_readiness(pin.manifest_generation, materialized)?;
        let searcher =
            self.acquire_lexical(&pin.repo_id, &pin.revision_id, pin.manifest_generation)?;
        // The symbol port has no count collector yet, so a `count` option
        // still yields a probe-derived (at-least) window here.
        budget.checkpoint("symbol:search")?;
        let mut results = searcher.search_symbols_constrained(
            &prepared_language.query,
            &prepared_language.constraints,
            probe_top_k_v1(request.top_k)?,
        )?;
        let window = finalize_probe_window_v1(&mut results, request.top_k)?;
        Ok(SymbolQueryResponse {
            generation: pin,
            results,
            window,
        })
    }

    /// Embed a semantic query string and gate it against the opened index's
    /// model identity. This is the single place the embed → model-identity-gate
    /// invariant lives for the semantic, hybrid, and hybrid-seed paths — a query
    /// vector from a model that differs from the indexed one is not
    /// cosine-comparable and must fail closed here.
    fn embed_and_gate_query(
        &self,
        query_text: &str,
        sem_searcher: &dyn SemanticSearcher,
        plane: &str,
    ) -> Result<Vec<f32>, CoreError> {
        let query_vector = self
            .query_embedder
            .embed_query(query_text)
            .map_err(|err| prefix_semantic_query_error(plane, err))?;
        ensure_query_model_matches_index_v1(
            self.query_embedder.model_id(),
            self.query_embedder.model_revision(),
            sem_searcher.index_model_id(),
            sem_searcher.index_model_revision(),
            plane,
        )?;
        Ok(query_vector)
    }

    /// The hybrid route: two independent, bounded lanes fused by RRF
    /// (QI-BB-018).
    ///
    /// The lexical lane runs the lowered text query; the dense lane runs the
    /// embedded semantic query over the whole generation under the same
    /// pushed-down constraints. Their union is fused, so a document the
    /// lexical lane never saw can enter the top-k on dense relevance alone —
    /// this is hybrid recall, not a dense re-rank of lexical recall.
    fn execute_hybrid_fusion(
        &self,
        selection: &SemanticSelection,
        text_query: &TextQueryRequest,
        semantic_query_text: &str,
        top_k: u32,
        plane: &str,
        budget: &RequestBudgetV1,
    ) -> Result<HybridFusion, CoreError> {
        let pin = selection.pin.clone();
        let lex_materialized = self.snapshot_lex_materialized(&pin.repo_id, &pin.revision_id)?;
        LexicalPolicy::validate_query_against_readiness(pin.manifest_generation, lex_materialized)?;
        self.validate_semantic_selection(selection, plane)?;

        let lex_searcher =
            self.acquire_lexical(&pin.repo_id, &pin.revision_id, pin.manifest_generation)?;
        let sem_searcher =
            self.acquire_semantic(&pin.repo_id, &pin.revision_id, pin.manifest_generation)?;
        let lexical_query = lower_lexical_text_query(text_query)?;
        let prepared_language = prepare_language_query_v1(lexical_query, &text_query.constraints)?;
        LexicalPolicy::validate_query(&prepared_language.query)?;
        let internal_top_k = hybrid_probe_top_k_v1(top_k)?;
        budget.checkpoint("hybrid:lexical")?;
        let mut lex_results = if prepared_language.force_empty {
            Vec::new()
        } else {
            lex_searcher
                .search_constrained(
                    &prepared_language.query,
                    &prepared_language.constraints,
                    internal_top_k,
                )?
                .candidates
        };
        stabilize_ranked_candidates(&mut lex_results);
        budget.checkpoint("hybrid:embed")?;
        let query_vector =
            self.embed_and_gate_query(semantic_query_text, sem_searcher.as_ref(), plane)?;
        budget.checkpoint("hybrid:semantic")?;
        // Independent dense lane under the same constraints, never scoped to
        // the lexical hits.
        let mut sem_results = if prepared_language.force_empty {
            Vec::new()
        } else {
            sem_searcher.search_constrained(
                &query_vector,
                &prepared_language.constraints,
                internal_top_k,
            )?
        };
        budget.checkpoint("hybrid:fuse")?;
        stabilize_ranked_candidates(&mut sem_results);
        let internal_limit = usize::try_from(internal_top_k).map_err(|err| {
            CoreError::InvalidContract(format!("hybrid: internal top_k overflow: {err}"))
        })?;
        let lane_limit_reached =
            lex_results.len() == internal_limit || sem_results.len() == internal_limit;
        let fused_universe_size = lex_results
            .iter()
            .map(|candidate| candidate.candidate_id.as_str())
            .chain(
                sem_results
                    .iter()
                    .map(|candidate| candidate.candidate_id.as_str()),
            )
            .collect::<BTreeSet<_>>()
            .len();
        let fused = HybridOrchestratorPolicy::fuse_rrf(&lex_results, &sem_results, top_k);
        let early_stop_reason = if fused_universe_size > fused.len() {
            Some(EarlyStopReason::CountReached)
        } else {
            None
        };
        let explanation = build_hybrid_response_explanation(
            lex_results.len(),
            sem_results.len(),
            fused_universe_size,
            fused.len(),
            internal_top_k,
            early_stop_reason,
            &sem_searcher.dense_lane(),
        );
        let window = fused_window_v1(top_k, fused.len(), fused_universe_size, lane_limit_reached)?;
        Ok(HybridFusion {
            pin,
            fused,
            window,
            explanation,
        })
    }

    fn semantic(
        &self,
        request: &SemanticQueryRequest,
        budget: &RequestBudgetV1,
    ) -> Result<SemanticQueryResponse, CoreError> {
        budget.checkpoint("semantic:entry")?;
        SemanticPolicy::validate_top_k(request.top_k)?;
        let selection =
            resolve_semantic_request_selection(self.activation_catalog.as_ref(), request)?;
        let pin = selection.pin.clone();
        self.validate_semantic_selection(&selection, "semantic")?;
        let mut effective_constraints = request.constraints.clone();
        let scope = if let Some(scope) = request.lexical_scope.as_ref() {
            // QI-BB-004: the scope's `top_k` is the lexical candidate cap the
            // contract promises. It is validated under the shared public
            // gate, the lexical lane is asked for exactly that many ranked
            // candidates, and the semantic allowlist is those ids and no
            // more — never a full-recall materialization of the scope query.
            let scope_cap = validate_query_top_k(scope.top_k)?;
            if scope.constraints != request.constraints {
                return Err(CoreError::InvalidContract(
                    "semantic: lexical scope constraints must equal outer semantic constraints"
                        .to_string(),
                ));
            }
            let lowered_scope = lower_lexical_text_query(scope)?;
            let prepared_language = prepare_language_query_v1(lowered_scope, &request.constraints)?;
            LexicalPolicy::validate_query(&prepared_language.query)?;
            effective_constraints = prepared_language.constraints.clone();
            let lex_materialized =
                self.snapshot_lex_materialized(&pin.repo_id, &pin.revision_id)?;
            LexicalPolicy::validate_query_against_readiness(
                pin.manifest_generation,
                lex_materialized,
            )?;
            let searcher =
                self.acquire_lexical(&pin.repo_id, &pin.revision_id, pin.manifest_generation)?;
            budget.checkpoint("semantic:scope")?;
            let mut scoped = if prepared_language.force_empty {
                Vec::new()
            } else {
                searcher
                    .search_constrained(
                        &prepared_language.query,
                        &prepared_language.constraints,
                        scope_cap,
                    )?
                    .candidates
            };
            if scoped.len() > top_k_limit(scope_cap) {
                return Err(CoreError::InvalidContract(format!(
                    "semantic: lexical scope adapter returned {} candidates for a cap of {scope_cap}",
                    scoped.len()
                )));
            }
            stabilize_ranked_candidates(&mut scoped);
            Some(SemanticScopeV1 {
                requested_cap: scope_cap,
                candidate_ids: scoped
                    .into_iter()
                    .map(|candidate| candidate.candidate_id)
                    .collect::<BTreeSet<_>>(),
            })
        } else {
            None
        };
        let scope_candidate_ids = scope.as_ref().map(|scope| &scope.candidate_ids);
        let searcher =
            self.acquire_semantic(&pin.repo_id, &pin.revision_id, pin.manifest_generation)?;
        budget.checkpoint("semantic:embed")?;
        let query_vector =
            self.embed_and_gate_query(request.query_text.as_str(), searcher.as_ref(), "semantic")?;
        let probe_top_k = probe_top_k_v1(request.top_k)?;
        budget.checkpoint("semantic:search")?;
        let mut results = if let Some(scope_ids) = scope_candidate_ids {
            searcher.search_scoped_constrained(
                &query_vector,
                scope_ids,
                &effective_constraints,
                probe_top_k,
            )?
        } else {
            searcher.search_constrained(&query_vector, &effective_constraints, probe_top_k)?
        };
        budget.checkpoint("semantic:project")?;
        let window = finalize_probe_window_v1(&mut results, request.top_k)?;
        let early_stop_reason = scope_candidate_ids.and_then(|scope_ids| {
            let limit = top_k_limit(request.top_k);
            if scope_ids.len() > results.len() && results.len() == limit {
                Some(EarlyStopReason::CountReached)
            } else {
                None
            }
        });
        let explanation = build_semantic_response_explanation(
            scope.as_ref(),
            results.len(),
            early_stop_reason,
            &searcher.dense_lane(),
        );
        Ok(SemanticQueryResponse {
            generation: pin,
            results,
            window,
            explanation,
        })
    }

    fn hybrid(
        &self,
        request: &HybridQueryRequest,
        budget: &RequestBudgetV1,
    ) -> Result<HybridQueryResponse, CoreError> {
        budget.checkpoint("hybrid:entry")?;
        HybridOrchestratorPolicy::validate_top_k(request.top_k)?;
        let selection =
            resolve_hybrid_request_selection(self.activation_catalog.as_ref(), request)?;
        let fusion = self.execute_hybrid_fusion(
            &selection,
            &request.text_query,
            request.semantic_query_text.as_str(),
            request.top_k,
            "hybrid",
            budget,
        )?;
        Ok(HybridQueryResponse {
            generation: fusion.pin,
            results: fusion.fused,
            window: fusion.window,
            explanation: fusion.explanation,
        })
    }

    fn hybrid_seed(
        &self,
        request: &HybridSeedQueryRequest,
        budget: &RequestBudgetV1,
    ) -> Result<HybridSeedQueryResponse, CoreError> {
        budget.checkpoint("hybrid-seed:entry")?;
        HybridOrchestratorPolicy::validate_top_k(request.top_k)?;
        let selection =
            resolve_hybrid_seed_request_selection(self.activation_catalog.as_ref(), request)?;
        let pin = selection.pin.clone();
        let lex_materialized = self.snapshot_lex_materialized(&pin.repo_id, &pin.revision_id)?;
        LexicalPolicy::validate_query_against_readiness(pin.manifest_generation, lex_materialized)?;
        let manifest_digest = self.validated_semantic_manifest_digest(&selection, "hybrid seed")?;
        let lex_searcher =
            self.acquire_lexical(&pin.repo_id, &pin.revision_id, pin.manifest_generation)?;
        let sem_searcher =
            self.acquire_semantic(&pin.repo_id, &pin.revision_id, pin.manifest_generation)?;
        let lexical_query = lower_lexical_text_query(&request.text_query)?;
        let prepared_language =
            prepare_language_query_v1(lexical_query, &request.text_query.constraints)?;
        LexicalPolicy::validate_query(&prepared_language.query)?;
        let internal_top_k = hybrid_probe_top_k_v1(request.top_k)?;
        budget.checkpoint("hybrid-seed:lexical")?;
        let mut lex_results = if prepared_language.force_empty {
            Vec::new()
        } else {
            lex_searcher
                .search_constrained(
                    &prepared_language.query,
                    &prepared_language.constraints,
                    internal_top_k,
                )?
                .candidates
        };
        stabilize_ranked_candidates(&mut lex_results);
        budget.checkpoint("hybrid-seed:embed")?;
        let query_vector = self.embed_and_gate_query(
            request.semantic_query_text.as_str(),
            sem_searcher.as_ref(),
            "hybrid seed",
        )?;
        let internal_limit = usize::try_from(internal_top_k).map_err(|err| {
            CoreError::InvalidContract(format!("hybrid seed: internal top_k overflow: {err}"))
        })?;
        let primary_lane_limit_reached = lex_results.len() == internal_limit;
        // One dense lane per requested corpus (or one global lane), each a
        // single native search over the query vector (QI-BB-019): there is
        // no second, lexical-scoped dense search behind the seed list.
        let dense_corpora = canonical_dense_corpus_budgets_v1(&request.dense_corpora)?;
        let mut unavailable_corpus_reasons = Vec::new();
        let mut semantic_lanes = Vec::new();
        if prepared_language.force_empty {
            semantic_lanes.push(Vec::new());
        } else if dense_corpora.is_empty() {
            budget.checkpoint("hybrid-seed:dense")?;
            let mut hits = sem_searcher.search_hits_constrained(
                &query_vector,
                &prepared_language.constraints,
                internal_top_k,
            )?;
            stabilize_semantic_seed_hits_v1(&mut hits);
            semantic_lanes.push(hits);
        } else {
            for corpus_budget in dense_corpora {
                // One dense lane per requested corpus; each is its own
                // native call, so each gets its own checkpoint.
                budget.checkpoint("hybrid-seed:dense")?;
                let mut hits = sem_searcher.search_hits_for_corpus_constrained(
                    &query_vector,
                    corpus_budget.corpus_kind,
                    &prepared_language.constraints,
                    corpus_budget.top_k,
                )?;
                stabilize_semantic_seed_hits_v1(&mut hits);
                if hits.is_empty() {
                    unavailable_corpus_reasons.push(format!(
                        "requested_semantic_corpus_unavailable:{}",
                        corpus_budget.corpus_kind.as_code_str()
                    ));
                }
                semantic_lanes.push(hits);
            }
        }
        budget.checkpoint("hybrid-seed:fuse")?;
        let semantic_hits = semantic_lanes.iter().flatten().collect::<Vec<_>>();
        let lexical_entity_count = lex_results
            .iter()
            .map(|candidate| candidate.candidate_id.as_str())
            .collect::<BTreeSet<_>>()
            .len();
        let semantic_entity_count = semantic_hits
            .iter()
            .map(|hit| hit.owner_id.as_str())
            .collect::<BTreeSet<_>>()
            .len();
        let fused_entity_universe = lex_results
            .iter()
            .map(|candidate| candidate.candidate_id.as_str())
            .chain(semantic_hits.iter().map(|hit| hit.owner_id.as_str()))
            .collect::<BTreeSet<_>>()
            .len();
        let seed_candidates = build_hybrid_seed_candidates(
            &lex_results,
            &semantic_lanes,
            &unavailable_corpus_reasons,
            request.top_k,
        )?;
        let early_stop_reason = if fused_entity_universe > seed_candidates.len() {
            Some(EarlyStopReason::CountReached)
        } else {
            None
        };
        let explanation = build_hybrid_seed_response_explanation(
            &SeedLaneTallyV1 {
                lexical_hits: lex_results.len(),
                lexical_entities: lexical_entity_count,
                semantic_hits: semantic_hits.len(),
                semantic_entities: semantic_entity_count,
                fused_hits: seed_candidates.len(),
            },
            internal_top_k,
            &unavailable_corpus_reasons,
            early_stop_reason,
            &sem_searcher.dense_lane(),
        );
        let window = fused_window_v1(
            request.top_k,
            seed_candidates.len(),
            fused_entity_universe,
            primary_lane_limit_reached,
        )?;
        Ok(HybridSeedQueryResponse {
            generation: pin,
            manifest_digest,
            seed_candidates,
            window,
            explanation,
        })
    }

    fn runtime_metadata(
        &self,
        request: &RuntimeMetadataQueryRequest,
        budget: &RequestBudgetV1,
    ) -> Result<SearchPlaneRuntimeMetadataQueryResponse, CoreError> {
        budget.checkpoint("runtime-metadata:entry")?;
        let _accepted_top_k = validate_query_top_k(request.text_query.top_k)?;
        let lowered = lower_lexical_text_query(&request.text_query)?;
        validate_runtime_metadata_query(&lowered)?;
        let pin = resolve_optional_selection(
            self.activation_catalog.as_ref(),
            request.text_query.generation.clone(),
            request.text_query.generation_selector.as_ref(),
            SearchPlaneTrackKind::Lexical,
            "runtime metadata",
        )?
        .ok_or_else(|| {
            CoreError::InvalidContract("runtime metadata: generation selector required".to_string())
        })?;
        // Snapshots are cloned under the read lock and scanned outside it
        // (QI-BB-020): a long scan never holds up an ingest, and an ingest
        // never holds up a query.
        let (runtime_state, structural_state) = {
            let guard = self.ledger.read().map_err(|_poisoned| {
                CoreError::Storage("search-plane ledger poisoned".to_string())
            })?;
            let snapshots = (
                guard.runtime_snapshot(&pin.repo_id, &pin.revision_id, pin.manifest_generation),
                guard.structural_snapshot(&pin.repo_id, &pin.revision_id, pin.manifest_generation),
            );
            drop(guard);
            snapshots
        };
        let runtime_state = runtime_state.ok_or_else(|| {
            CoreError::NotReady(format!(
                "runtime metadata: generation {} is not materialized",
                pin.manifest_generation.get()
            ))
        })?;
        let structural_state = structural_state.ok_or_else(|| {
            CoreError::NotReady(format!(
                "runtime metadata: lexical chunk authority for generation {} is not materialized",
                pin.manifest_generation.get()
            ))
        })?;
        if runtime_query_requires_catalog(&lowered) {
            ensure_runtime_catalog_ready(&runtime_state)?;
            ensure_runtime_snapshot_names_known(&lowered, &runtime_state)?;
        }
        budget.checkpoint("runtime-metadata:execute")?;
        let mut results = execute_runtime_metadata_query(
            &pin,
            &lowered,
            &runtime_state,
            &structural_state,
            request.text_query.top_k,
        )?;
        stabilize_ranked_candidates(&mut results);
        Ok(SearchPlaneRuntimeMetadataQueryResponse {
            generation: pin,
            results,
        })
    }

    fn history(
        &self,
        request: &HistoryQueryRequest,
        budget: &RequestBudgetV1,
    ) -> Result<SearchPlaneHistoryQueryResponse, CoreError> {
        budget.checkpoint("history:entry")?;
        let _accepted_top_k = validate_query_top_k(request.text_query.top_k)?;
        let lowered = lower_lexical_text_query(&request.text_query)?;
        validate_history_query(&lowered)?;
        let pin = resolve_optional_selection(
            self.activation_catalog.as_ref(),
            request.text_query.generation.clone(),
            request.text_query.generation_selector.as_ref(),
            SearchPlaneTrackKind::Lexical,
            "history",
        )?
        .ok_or_else(|| {
            CoreError::InvalidContract("history: generation selector required".to_string())
        })?;
        let history_state = {
            let guard = self.ledger.read().map_err(|_poisoned| {
                CoreError::Storage("search-plane ledger poisoned".to_string())
            })?;
            resolve_history_state(&guard, &pin, &lowered)?
        };
        budget.checkpoint("history:execute")?;
        let page = execute_history_query(
            &lowered,
            &history_state,
            request.text_query.top_k,
            request.cursor.as_ref(),
        )?;
        Ok(SearchPlaneHistoryQueryResponse {
            generation: pin,
            commits: page.commits,
            diffs: page.diffs,
            window: page.window,
            examined: page.examined,
            next_cursor: page.next_cursor,
        })
    }

    fn structural(
        &self,
        request: &StructuralQueryRequest,
        budget: &RequestBudgetV1,
    ) -> Result<SearchPlaneStructuralQueryResponse, CoreError> {
        budget.checkpoint("structural:entry")?;
        let _accepted_top_k = validate_query_top_k(request.text_query.top_k)?;
        let (pin, lowered) =
            lower_structural_query_request(self.activation_catalog.as_ref(), request)?;
        let results =
            self.execute_structural_results(&pin, &lowered, request.text_query.top_k, budget)?;
        Ok(SearchPlaneStructuralQueryResponse {
            generation: pin,
            results,
        })
    }

    /// Explain one candidate (QI-BB-022): an exact presence lookup, and when
    /// the request names the query, the score the lexical engine emits for
    /// exactly this candidate under the plan that ranked it.
    fn explain(
        &self,
        request: SearchPlaneExplainQueryRequest,
        budget: &RequestBudgetV1,
    ) -> Result<SearchPlaneExplainQueryResponse, CoreError> {
        budget.checkpoint("explain:entry")?;
        let pin = request.generation;
        if request.candidate.manifest_generation != pin.manifest_generation {
            return Err(CoreError::InvalidContract(format!(
                "explain: candidate manifest_generation {} != pin {}",
                request.candidate.manifest_generation.get(),
                pin.manifest_generation.get()
            )));
        }
        if request.candidate.repo_id != pin.repo_id
            || request.candidate.revision_id != pin.revision_id
        {
            return Err(CoreError::InvalidContract(
                "explain: candidate (repo, revision) does not match pin".to_string(),
            ));
        }
        let materialized = self.snapshot_lex_materialized(&pin.repo_id, &pin.revision_id)?;
        LexicalPolicy::validate_query_against_readiness(pin.manifest_generation, materialized)?;
        let searcher =
            self.acquire_lexical(&pin.repo_id, &pin.revision_id, pin.manifest_generation)?;
        let candidate_id = request.candidate.candidate_id.as_str();
        let Some(text_query) = request.text_query else {
            budget.checkpoint("explain:presence")?;
            let presence = searcher.candidate_presence(candidate_id)?;
            return Ok(SearchPlaneExplainQueryResponse {
                generation: pin,
                presence,
                explanation: build_presence_explanation(candidate_id, presence),
            });
        };
        // The query names its generation at most once, and it is this one.
        if text_query.generation_selector.is_some() {
            return Err(CoreError::InvalidContract(
                "explain: text_query must not carry a generation selector; the explain pins its generation"
                    .to_string(),
            ));
        }
        if text_query
            .generation
            .as_ref()
            .is_some_and(|query_pin| *query_pin != pin)
        {
            return Err(CoreError::InvalidContract(
                "explain: text_query generation does not match the explain pin".to_string(),
            ));
        }
        // The query is the one the search accepted, `top_k` included; the
        // trace does not page, but it does not accept a request the search
        // would have refused either.
        let _accepted_top_k = validate_query_top_k(text_query.top_k)?;
        let pinned_query = TextQueryRequest {
            generation: Some(pin.clone()),
            ..text_query
        };
        budget.checkpoint("explain:plan")?;
        let planned = self.plan_lexical_text_query(&pinned_query)?;
        if planned.pin != pin {
            return Err(CoreError::InvalidContract(format!(
                "explain: the query rebinds to generation {} but the candidate is at {}",
                planned.pin.manifest_generation.get(),
                pin.manifest_generation.get()
            )));
        }
        budget.checkpoint("explain:score")?;
        let explained = if planned.force_empty {
            match searcher.candidate_presence(candidate_id)? {
                CandidatePresenceV1::Indexed => LexicalCandidateExplanationV1::NotMatched {
                    reason: "the plan is a contradiction and matches nothing".to_string(),
                },
                CandidatePresenceV1::NotIndexed => LexicalCandidateExplanationV1::NotIndexed,
            }
        } else {
            searcher.explain_candidate(&planned.query, &planned.constraints, candidate_id)?
        };
        let presence = match explained {
            LexicalCandidateExplanationV1::NotIndexed => CandidatePresenceV1::NotIndexed,
            LexicalCandidateExplanationV1::NotMatched { .. }
            | LexicalCandidateExplanationV1::Matched(_) => CandidatePresenceV1::Indexed,
        };
        let explanation = build_lexical_score_explanation(
            candidate_id,
            request.candidate.score,
            &planned.query.options,
            &explained,
        )?;
        Ok(SearchPlaneExplainQueryResponse {
            generation: pin,
            presence,
            explanation,
        })
    }

    fn repo_map(
        &self,
        request: RepoMapQueryRequest,
        budget: &RequestBudgetV1,
    ) -> Result<RepoMapQueryResponse, CoreError> {
        budget.checkpoint("repo-map:entry")?;
        RepoMapPolicy::validate_query(&request)?;
        self.repo_map_query.query(request)
    }

    fn execute_structural_results(
        &self,
        pin: &GenerationPin,
        lowered: &LqQuery,
        top_k: u32,
        budget: &RequestBudgetV1,
    ) -> Result<Vec<quanta_index_contract::StructuralCandidate>, CoreError> {
        if !structural_expr_has_structural_leaf(&lowered.expr) {
            return Err(structural_invalid_request(
                "query must include at least one structural `match { ... }` leaf",
            ));
        }
        let has_lexical = structural_expr_has_non_structural_leaf(&lowered.expr);
        let requested_lang = extract_structural_requested_lang(&lowered.expr, has_lexical)?;
        let (requested_lang, executable_filters) =
            extract_structural_filters(lowered, requested_lang.as_deref())?;
        let seed = if structural_expr_is_pure_negative_root(&lowered.expr) {
            let structural_state = self
                .ledger
                .read()
                .map_err(|_poisoned| {
                    CoreError::Storage("search-plane ledger poisoned".to_string())
                })?
                .structural_snapshot(&pin.repo_id, &pin.revision_id, pin.manifest_generation)
                .ok_or_else(|| {
                    CoreError::NotReady(format!(
                        "structural: generation {} chunk authority is not materialized",
                        pin.manifest_generation.get()
                    ))
                })?;
            Some(build_pinned_structural_universe(
                pin,
                &structural_state,
                requested_lang.as_deref(),
                &executable_filters,
            )?)
        } else {
            None
        };
        let service = StructuralService::new(Arc::clone(&self.structural_producer));
        let mut ctx = StructuralEvalContext::default();
        let lexical_eval = if has_lexical {
            Some(LexicalSubexprEvaluator {
                dispatcher: self,
                pin,
                query: lowered,
                budget,
            })
        } else {
            None
        };
        budget.checkpoint("structural:execute")?;
        let candidates = evaluate_structural_expr(
            &mut ctx,
            &service,
            pin,
            &lowered.expr,
            requested_lang.as_deref(),
            &executable_filters,
            &lowered.options,
            seed.as_ref(),
            lexical_eval.as_ref(),
        )?;
        let mut results = project_structural_query_results(candidates);
        results.truncate(top_k_limit(top_k));
        Ok(results)
    }

    /// Serve one query under its request budget (QI-BB-002).
    ///
    /// The budget's deadline and cancellation are observed at every lane
    /// boundary a route owns; a request that runs past either is answered
    /// with a typed `REQUEST_DEADLINE_EXCEEDED` / `REQUEST_CANCELLED` naming
    /// the checkpoint that saw it. The native call between two checkpoints
    /// always runs to completion (G0-R).
    #[must_use]
    pub fn dispatch(
        &self,
        request: SearchPlaneQueryIpcRequest,
        budget: &RequestBudgetV1,
    ) -> SearchPlaneQueryIpcResponse {
        // One arm per variant; each delegates to a private handler that
        // returns the already-wrapped `SearchPlaneQueryIpcResponse`. Adding a
        // new variant means: add one handler fn + add one match arm — no
        // edits to encode/decode/match/factory all at once.
        match request {
            SearchPlaneQueryIpcRequest::Text(req) => self.dispatch_text(req, budget),
            SearchPlaneQueryIpcRequest::Symbol(req) => self.dispatch_symbol(req, budget),
            SearchPlaneQueryIpcRequest::Semantic(req) => self.dispatch_semantic(req, budget),
            SearchPlaneQueryIpcRequest::Hybrid(req) => self.dispatch_hybrid(req, budget),
            SearchPlaneQueryIpcRequest::HybridSeed(req) => self.dispatch_hybrid_seed(&req, budget),
            SearchPlaneQueryIpcRequest::History(req) => self.dispatch_history(&req, budget),
            SearchPlaneQueryIpcRequest::Structural(req) => self.dispatch_structural(&req, budget),
            SearchPlaneQueryIpcRequest::RepoMapQuery(req) => self.dispatch_repo_map(req, budget),
            SearchPlaneQueryIpcRequest::Explain(req) => self.dispatch_explain(req, budget),
            SearchPlaneQueryIpcRequest::RuntimeMetadata(req) => {
                self.dispatch_runtime_metadata(&req, budget)
            }
            SearchPlaneQueryIpcRequest::ClusterMembershipRead(req) => {
                self.dispatch_cluster_membership_batch_read(&req, budget)
            }
        }
    }

    fn dispatch_cluster_membership_batch_read(
        &self,
        request: &ClusterMembershipBatchReadRequestV1,
        budget: &RequestBudgetV1,
    ) -> SearchPlaneQueryIpcResponse {
        match self.cluster_membership_batch_read(request, budget) {
            Ok(outcome) => SearchPlaneQueryIpcResponse::ClusterMembershipRead(outcome),
            Err(error) => SearchPlaneQueryIpcResponse::Error(core_error_to_ipc(error)),
        }
    }

    pub fn cluster_membership_batch_read(
        &self,
        request: &ClusterMembershipBatchReadRequestV1,
        budget: &RequestBudgetV1,
    ) -> Result<ClusterMembershipBatchReadResponseV1, CoreError> {
        budget.checkpoint("cluster-membership:entry")?;
        request
            .validate_v1()
            .map_err(|error| CoreError::InvalidContract(error.to_string()))?;
        {
            let ledger = self
                .ledger
                .read()
                .map_err(|error| CoreError::Storage(format!("ledger poisoned: {error}")))?;
            ledger.validate_semantic_generation(
                &request.generation.repo_id,
                &request.generation.revision_id,
                request.generation.manifest_generation,
                None,
                true,
                "cluster membership read",
            )?;
        }
        let searcher = self.acquire_semantic(
            &request.generation.repo_id,
            &request.generation.revision_id,
            request.generation.manifest_generation,
        )?;
        budget.checkpoint("cluster-membership:read")?;
        let outcome = searcher.cluster_membership_batch_read(request)?;
        outcome.validate_against_v1(request).map_err(|failure| {
            CoreError::InvalidContract(format!(
                "cluster membership batch read: searcher returned invalid authority: {failure}"
            ))
        })?;
        Ok(outcome)
    }

    fn dispatch_text(
        &self,
        request: TextQueryRequest,
        budget: &RequestBudgetV1,
    ) -> SearchPlaneQueryIpcResponse {
        let requested_pin = request.generation.clone();
        self.emit_intake_metric(requested_pin.as_ref());
        match self.lexical_query(request, budget) {
            Ok(response) => {
                self.emit_planner_metric(&response.generation);
                self.emit_engine_fanout_metric(&response.generation, 1);
                SearchPlaneQueryIpcResponse::Text(response)
            }
            Err(err) => {
                self.emit_error_metric(requested_pin.as_ref(), &err);
                SearchPlaneQueryIpcResponse::Error(core_error_to_ipc(err))
            }
        }
    }

    fn dispatch_symbol(
        &self,
        request: SymbolQueryRequest,
        budget: &RequestBudgetV1,
    ) -> SearchPlaneQueryIpcResponse {
        let requested_pin = request.generation.clone();
        self.emit_intake_metric(requested_pin.as_ref());
        match self.symbol(request, budget) {
            Ok(response) => {
                self.emit_planner_metric(&response.generation);
                self.emit_engine_fanout_metric(&response.generation, 1);
                SearchPlaneQueryIpcResponse::Symbol(response)
            }
            Err(err) => {
                self.emit_error_metric(requested_pin.as_ref(), &err);
                SearchPlaneQueryIpcResponse::Error(core_error_to_ipc(err))
            }
        }
    }

    fn dispatch_semantic(
        &self,
        request: SemanticQueryRequest,
        budget: &RequestBudgetV1,
    ) -> SearchPlaneQueryIpcResponse {
        let requested_pin = request.generation.clone();
        self.emit_intake_metric(requested_pin.as_ref());
        match self.semantic_query(request, budget) {
            Ok(response) => {
                self.emit_planner_metric(&response.generation);
                self.emit_engine_fanout_metric(
                    &response.generation,
                    response.explanation.engines_touched.len(),
                );
                self.emit_early_stop_metric(
                    &response.generation,
                    response.explanation.early_stop_reason,
                );
                SearchPlaneQueryIpcResponse::Semantic(response)
            }
            Err(err) => {
                self.emit_error_metric(requested_pin.as_ref(), &err);
                SearchPlaneQueryIpcResponse::Error(core_error_to_ipc(err))
            }
        }
    }

    fn dispatch_hybrid(
        &self,
        request: HybridQueryRequest,
        budget: &RequestBudgetV1,
    ) -> SearchPlaneQueryIpcResponse {
        let requested_pin = request.text_query.generation.clone();
        self.emit_intake_metric(requested_pin.as_ref());
        match self.hybrid_query(request, budget) {
            Ok(response) => {
                self.emit_planner_metric(&response.generation);
                self.emit_engine_fanout_metric(
                    &response.generation,
                    response.explanation.engines_touched.len(),
                );
                self.emit_merge_count_metric(&response.generation, response.results.len());
                self.emit_early_stop_metric(
                    &response.generation,
                    response.explanation.early_stop_reason,
                );
                SearchPlaneQueryIpcResponse::Hybrid(response)
            }
            Err(err) => {
                self.emit_error_metric(requested_pin.as_ref(), &err);
                SearchPlaneQueryIpcResponse::Error(core_error_to_ipc(err))
            }
        }
    }

    fn dispatch_hybrid_seed(
        &self,
        request: &HybridSeedQueryRequest,
        budget: &RequestBudgetV1,
    ) -> SearchPlaneQueryIpcResponse {
        let requested_pin = request.text_query.generation.as_ref();
        self.emit_intake_metric(requested_pin);
        match self.hybrid_seed(request, budget) {
            Ok(response) => {
                self.emit_planner_metric(&response.generation);
                self.emit_engine_fanout_metric(
                    &response.generation,
                    response.explanation.engines_touched.len(),
                );
                // The merge count is what the window says the page holds
                // (QI-BB-019): the one canonical seed list.
                self.emit_merge_count_metric(
                    &response.generation,
                    usize::try_from(response.window.returned()).map_or(usize::MAX, |n| n),
                );
                self.emit_early_stop_metric(
                    &response.generation,
                    response.explanation.early_stop_reason,
                );
                SearchPlaneQueryIpcResponse::HybridSeed(response)
            }
            Err(err) => {
                self.emit_error_metric(requested_pin, &err);
                SearchPlaneQueryIpcResponse::Error(core_error_to_ipc(err))
            }
        }
    }

    fn dispatch_history(
        &self,
        request: &HistoryQueryRequest,
        budget: &RequestBudgetV1,
    ) -> SearchPlaneQueryIpcResponse {
        let requested_pin = request.text_query.generation.as_ref();
        self.emit_intake_metric(requested_pin);
        match self.history(request, budget) {
            Ok(response) => {
                self.emit_planner_metric(&response.generation);
                self.emit_engine_fanout_metric(&response.generation, 1);
                self.emit_merge_count_metric(
                    &response.generation,
                    response.commits.len().saturating_add(response.diffs.len()),
                );
                SearchPlaneQueryIpcResponse::History(response)
            }
            Err(err) => {
                self.emit_error_metric(requested_pin, &err);
                SearchPlaneQueryIpcResponse::Error(core_error_to_ipc(err))
            }
        }
    }

    fn dispatch_structural(
        &self,
        request: &StructuralQueryRequest,
        budget: &RequestBudgetV1,
    ) -> SearchPlaneQueryIpcResponse {
        let requested_pin = request.text_query.generation.as_ref();
        self.emit_intake_metric(requested_pin);
        match self.structural(request, budget) {
            Ok(response) => {
                self.emit_planner_metric(&response.generation);
                self.emit_engine_fanout_metric(&response.generation, 1);
                self.emit_merge_count_metric(&response.generation, response.results.len());
                SearchPlaneQueryIpcResponse::Structural(response)
            }
            Err(err) => {
                self.emit_error_metric(requested_pin, &err);
                SearchPlaneQueryIpcResponse::Error(core_error_to_ipc(err))
            }
        }
    }

    fn dispatch_repo_map(
        &self,
        request: RepoMapQueryRequest,
        budget: &RequestBudgetV1,
    ) -> SearchPlaneQueryIpcResponse {
        let requested_pin = GenerationPin::new(
            request.repo_id.clone(),
            request.revision_id.clone(),
            request.manifest_generation,
        );
        self.emit_intake_metric(Some(&requested_pin));
        match self.repo_map(request, budget) {
            Ok(response) => {
                let response_pin = GenerationPin::new(
                    response.repo_id.clone(),
                    response.revision_id.clone(),
                    response.manifest_generation,
                );
                self.emit_planner_metric(&response_pin);
                self.emit_engine_fanout_metric(&response_pin, 1);
                self.emit_merge_count_metric(&response_pin, response.entries.len());
                SearchPlaneQueryIpcResponse::RepoMapQuery(response)
            }
            Err(err) => {
                self.emit_error_metric(Some(&requested_pin), &err);
                SearchPlaneQueryIpcResponse::Error(core_error_to_ipc(err))
            }
        }
    }

    fn dispatch_explain(
        &self,
        request: SearchPlaneExplainQueryRequest,
        budget: &RequestBudgetV1,
    ) -> SearchPlaneQueryIpcResponse {
        let requested_pin = request.generation.clone();
        self.emit_intake_metric(Some(&requested_pin));
        match self.explain_query(request, budget) {
            Ok(response) => {
                self.emit_planner_metric(&response.generation);
                self.emit_engine_fanout_metric(
                    &response.generation,
                    response.explanation.engines_touched.len(),
                );
                self.emit_early_stop_metric(
                    &response.generation,
                    response.explanation.early_stop_reason,
                );
                SearchPlaneQueryIpcResponse::Explain(response)
            }
            Err(err) => {
                self.emit_error_metric(Some(&requested_pin), &err);
                SearchPlaneQueryIpcResponse::Error(core_error_to_ipc(err))
            }
        }
    }

    // QI-RT-02 (in-flight): runtime-metadata query path is defined in the
    // contract but the producer-backed implementation is not wired yet.
    // Fail-closed with a dedicated typed-unavailable code.
    fn dispatch_runtime_metadata(
        &self,
        request: &RuntimeMetadataQueryRequest,
        budget: &RequestBudgetV1,
    ) -> SearchPlaneQueryIpcResponse {
        let requested_pin = request.text_query.generation.as_ref();
        self.emit_intake_metric(requested_pin);
        match self.runtime_metadata(request, budget) {
            Ok(response) => {
                self.emit_planner_metric(&response.generation);
                self.emit_engine_fanout_metric(&response.generation, 1);
                self.emit_merge_count_metric(&response.generation, response.results.len());
                SearchPlaneQueryIpcResponse::RuntimeMetadata(response)
            }
            Err(err) => {
                self.emit_error_metric(requested_pin, &err);
                SearchPlaneQueryIpcResponse::Error(core_error_to_ipc(err))
            }
        }
    }

    fn snapshot_lex_materialized(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
    ) -> Result<Option<ManifestGeneration>, CoreError> {
        let guard = self
            .ledger
            .read()
            .map_err(|err| CoreError::Storage(format!("ledger poisoned: {err}")))?;
        Ok(guard.track_materialized(repo_id, revision_id, SearchPlaneTrackKind::Lexical))
    }

    /// The structural snapshot of the pinned generation, shared rather
    /// than copied (QI-BB-020).
    fn snapshot_structural_state(
        &self,
        pin: &GenerationPin,
    ) -> Result<Arc<StructuralAuthorityState>, CoreError> {
        let guard = self
            .ledger
            .read()
            .map_err(|err| CoreError::Storage(format!("ledger poisoned: {err}")))?;
        guard
            .structural_snapshot(&pin.repo_id, &pin.revision_id, pin.manifest_generation)
            .ok_or_else(|| {
                CoreError::NotReady(format!(
                    "structural: generation {} chunk authority is not materialized",
                    pin.manifest_generation.get()
                ))
            })
    }

    fn validate_semantic_selection(
        &self,
        selection: &SemanticSelection,
        plane: &str,
    ) -> Result<(), CoreError> {
        let guard = self
            .ledger
            .read()
            .map_err(|err| CoreError::Storage(format!("ledger poisoned: {err}")))?;
        guard.validate_semantic_generation(
            &selection.pin.repo_id,
            &selection.pin.revision_id,
            selection.pin.manifest_generation,
            selection.expected_manifest_digest.as_deref(),
            true,
            plane,
        )
    }

    fn validated_semantic_manifest_digest(
        &self,
        selection: &SemanticSelection,
        plane: &str,
    ) -> Result<String, CoreError> {
        let guard = self
            .ledger
            .read()
            .map_err(|err| CoreError::Storage(format!("ledger poisoned: {err}")))?;
        guard.validate_semantic_generation(
            &selection.pin.repo_id,
            &selection.pin.revision_id,
            selection.pin.manifest_generation,
            selection.expected_manifest_digest.as_deref(),
            true,
            plane,
        )?;
        guard
            .semantic_generation_state(
                &selection.pin.repo_id,
                &selection.pin.revision_id,
                selection.pin.manifest_generation,
            )
            .map(|state| state.manifest_digest().to_string())
            .ok_or_else(|| {
                CoreError::NotReady(format!(
                    "{plane}: semantic generation {} manifest authority disappeared after validation",
                    selection.pin.manifest_generation.get()
                ))
            })
    }

    fn emit_metric(
        &self,
        pin: Option<&GenerationPin>,
        name: &'static str,
        kind: MetricKind,
        value: f64,
    ) {
        let (repo_id, generation_id) = pin.map_or(("unresolved", 0), |pin| {
            (pin.repo_id.as_str(), pin.manifest_generation.get())
        });
        self.obs_sink.emit(MetricSample::new(
            name,
            kind,
            value,
            Dimensions::new("LXE-10", "8", "local", repo_id, generation_id),
        ));
    }

    fn emit_intake_metric(&self, pin: Option<&GenerationPin>) {
        self.emit_metric(pin, "lq_query_intake_total", MetricKind::Counter, 1.0);
    }

    fn emit_planner_metric(&self, pin: &GenerationPin) {
        self.emit_metric(Some(pin), "lq_planner_total", MetricKind::Counter, 1.0);
    }

    fn emit_engine_fanout_metric(&self, pin: &GenerationPin, count: usize) {
        self.emit_metric(
            Some(pin),
            "lq_engine_fanout_count",
            MetricKind::Histogram,
            metric_count_value(count),
        );
    }

    fn emit_merge_count_metric(&self, pin: &GenerationPin, count: usize) {
        self.emit_metric(
            Some(pin),
            "lq_merge_result_count",
            MetricKind::Histogram,
            metric_count_value(count),
        );
    }

    fn emit_early_stop_metric(&self, pin: &GenerationPin, reason: Option<EarlyStopReason>) {
        if reason.is_some() {
            self.emit_metric(Some(pin), "lq_early_stop_total", MetricKind::Counter, 1.0);
        }
    }

    fn emit_error_metric(&self, pin: Option<&GenerationPin>, err: &CoreError) {
        self.emit_metric(
            pin,
            classify_error_metric_name(err),
            MetricKind::Counter,
            1.0,
        );
    }
}

impl LexicalQueryPort for SearchPlaneDispatcher {
    fn lexical_query(
        &self,
        request: TextQueryRequest,
        budget: &RequestBudgetV1,
    ) -> Result<TextQueryResponse, CoreError> {
        self.lexical(&request, budget)
    }
}

impl SemanticQueryPort for SearchPlaneDispatcher {
    fn semantic_query(
        &self,
        request: SemanticQueryRequest,
        budget: &RequestBudgetV1,
    ) -> Result<SemanticQueryResponse, CoreError> {
        self.semantic(&request, budget)
    }
}

impl HybridQueryPort for SearchPlaneDispatcher {
    fn hybrid_query(
        &self,
        request: HybridQueryRequest,
        budget: &RequestBudgetV1,
    ) -> Result<HybridQueryResponse, CoreError> {
        self.hybrid(&request, budget)
    }
}

impl ExplainQueryPort for SearchPlaneDispatcher {
    fn explain_query(
        &self,
        request: SearchPlaneExplainQueryRequest,
        budget: &RequestBudgetV1,
    ) -> Result<SearchPlaneExplainQueryResponse, CoreError> {
        self.explain(request, budget)
    }
}

/// Test-only fail-closed structural producer.
///
/// Production runtime wiring uses the ledger-backed adapter in
/// `searchd::app::runtime`. This stand-in remains only for unit tests that
/// exercise unrelated query surfaces without materializing structural
/// authority.
#[cfg(test)]
struct FailClosedStructuralProducer;

#[cfg(test)]
impl StructuralProducerPort for FailClosedStructuralProducer {
    fn readiness(
        &self,
        _request: &DomainStructuralQueryRequest,
    ) -> quanta_index_core::domains::structural::StructuralReadiness {
        quanta_index_core::domains::structural::StructuralReadiness::ParseTreeProducerUnavailable
    }

    fn execute(
        &self,
        _request: &DomainStructuralQueryRequest,
    ) -> Result<
        Vec<quanta_index_core::StructuralMatchCandidate>,
        quanta_index_core::domains::structural::StructuralError,
    > {
        Err(quanta_index_core::domains::structural::StructuralError::ProducerExecution(
            "FailClosedStructuralProducer.execute should remain unreachable while readiness is ParseTreeProducerUnavailable".to_string(),
        ))
    }
}

fn core_error_to_ipc(err: CoreError) -> SearchPlaneIpcError {
    let (code, message) = match err {
        CoreError::InvalidContract(msg) => (ERR_INVALID.to_string(), msg),
        CoreError::Typed { code, message } => (code, message),
        CoreError::NotReady(msg) => (ERR_NOT_READY.to_string(), msg),
        CoreError::NotImplemented(msg) => (ERR_NOT_IMPLEMENTED.to_string(), msg),
        CoreError::NotFound(msg) => (ERR_NOT_FOUND.to_string(), msg),
        CoreError::Storage(msg) => (ERR_INTERNAL.to_string(), msg),
    };
    let repair = repair_for_code(&code);
    SearchPlaneIpcError {
        code,
        message,
        repair,
    }
}

/// Deterministic typed repair metadata for a wire error code (J7Q-06).
///
/// Advisory only — it never changes the fail-closed `code`/`message` outcome and
/// never rewrites the query. Each repairable code maps to its [`RepairClass`],
/// a set of *confirmed-supported* alternative filter shapes the caller can move
/// to, and a docs anchor pointing at the in-repo capability inventory. Internal
/// invariant breaks (e.g. `BRIDGE_TRANSLATE_FAIL`) and generic failures the
/// caller cannot act on return `None` rather than a misleading hint.
///
/// The alternative shapes are intentionally the small set verified to exist in
/// this plane (`repo:` / `file:` / `path:` / `lang:` / `rev:`); the anchor is the
/// authority for the full list.
///
/// `pub` so the J7Q-06 ambiguity rail can snapshot the exact payloads the wire
/// boundary emits without re-deriving the policy.
#[must_use]
pub fn repair_for_code(code: &str) -> Option<QueryErrorRepair> {
    const DOCS_ANCHOR: &str = "docs/analysis/jun-4-dsl-capabilty.md";
    let (class, alternatives): (RepairClass, &[&str]) = match code {
        c if c == LexicalErrorCode::BridgeAmbiguousFilter.as_code_str() => (
            RepairClass::Ambiguous,
            &["repo:<value>", "file:<value>", "path:<value>"],
        ),
        c if c == LexicalErrorCode::BridgeUnsupportedFilter.as_code_str() => (
            RepairClass::Unsupported,
            &["repo:<name>", "file:<glob>", "path:<glob>", "lang:<name>"],
        ),
        c if c == LexicalErrorCode::BridgeUnsupportedDirective.as_code_str() => (
            RepairClass::Unsupported,
            &["remove the directive", "use an explicit filter shape"],
        ),
        c if c == LexicalErrorCode::BridgeVersionPin.as_code_str() => (
            RepairClass::Malformed,
            &["rev:<git-ref>", "remove the version pin"],
        ),
        _ => return None,
    };
    Some(QueryErrorRepair {
        class,
        supported_alternatives: alternatives.iter().map(|s| (*s).to_string()).collect(),
        docs_anchor: Some(DOCS_ANCHOR.to_string()),
    })
}

#[cfg(test)]
mod repair_for_code_tests {
    use super::repair_for_code;
    use quanta_index_contract::RepairClass;
    use quanta_index_contract::lex::LexicalErrorCode;

    #[test]
    fn ambiguous_filter_maps_to_ambiguous_class() {
        let repair = repair_for_code(LexicalErrorCode::BridgeAmbiguousFilter.as_code_str())
            .expect("ambiguous filter is repairable");
        assert_eq!(repair.class, RepairClass::Ambiguous);
        assert!(!repair.supported_alternatives.is_empty());
        assert!(repair.docs_anchor.is_some());
    }

    #[test]
    fn unsupported_filter_and_directive_map_to_unsupported() {
        for code in [
            LexicalErrorCode::BridgeUnsupportedFilter,
            LexicalErrorCode::BridgeUnsupportedDirective,
        ] {
            let repair = repair_for_code(code.as_code_str())
                .unwrap_or_else(|| panic!("{} should be repairable", code.as_code_str()));
            assert_eq!(repair.class, RepairClass::Unsupported);
        }
    }

    #[test]
    fn version_pin_maps_to_malformed() {
        let repair = repair_for_code(LexicalErrorCode::BridgeVersionPin.as_code_str())
            .expect("version pin is repairable");
        assert_eq!(repair.class, RepairClass::Malformed);
    }

    #[test]
    fn internal_and_unknown_codes_have_no_repair() {
        // Translator invariant breaks are not user-repairable: no misleading hint.
        assert!(repair_for_code(LexicalErrorCode::BridgeTranslateFail.as_code_str()).is_none());
        assert!(repair_for_code("NOT_READY").is_none());
        assert!(repair_for_code("INTERNAL").is_none());
        assert!(repair_for_code("").is_none());
    }

    #[test]
    fn unsupported_filter_alternatives_are_confirmed_filter_families() {
        // Guard against drift into filter names this plane does not actually ship.
        let confirmed = ["repo:", "file:", "path:", "lang:", "rev:"];
        let repair = repair_for_code(LexicalErrorCode::BridgeUnsupportedFilter.as_code_str())
            .expect("repairable");
        for alt in &repair.supported_alternatives {
            assert!(
                confirmed.iter().any(|c| alt.starts_with(c)),
                "alternative `{alt}` is not a confirmed filter family"
            );
        }
    }
}

fn query_selects_file_owner_projection(query: &LqQuery) -> bool {
    query.filters.iter().any(|filter| {
        matches!(
            filter,
            LqFilter::Select {
                dim: quanta_index_contract::LqSelect::FileOwners
            }
        )
    })
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct ExecutableTextPlaneValidationState {
    saw_runtime_authority_filter: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ExecutableTextPlanePolicy {
    History,
    RuntimeMetadata,
}

impl ExecutableTextPlanePolicy {
    const fn plane_name(self) -> &'static str {
        match self {
            Self::History => "history",
            Self::RuntimeMetadata => "runtime metadata",
        }
    }

    fn validate_filter(
        self,
        filter: &LqFilter,
        state: &mut ExecutableTextPlaneValidationState,
    ) -> Result<(), CoreError> {
        match self {
            Self::History => match filter {
                LqFilter::Type { kind } => match kind {
                    LqType::Commit | LqType::Diff => Ok(()),
                    LqType::File | LqType::Path | LqType::Symbol | LqType::Repo => {
                        Err(CoreError::NotImplemented(format!(
                            "history: type filter `{}` is not executable on the current adapter set",
                            kind.as_str()
                        )))
                    }
                },
                LqFilter::File { .. }
                | LqFilter::Rev { .. }
                | LqFilter::Author { .. }
                | LqFilter::Committer { .. }
                | LqFilter::Message { .. }
                | LqFilter::Before { .. }
                | LqFilter::After { .. }
                | LqFilter::Since { .. }
                | LqFilter::Until { .. }
                | LqFilter::DiffAdded { .. }
                | LqFilter::DiffRemoved { .. }
                | LqFilter::DiffTouched { .. }
                | LqFilter::Content { .. } => Ok(()),
                LqFilter::Repo { .. }
                | LqFilter::Lang { .. }
                | LqFilter::Select { .. }
                | LqFilter::Dirty { .. }
                | LqFilter::Changed { .. }
                | LqFilter::Stale { .. }
                | LqFilter::Snapshot { .. }
                | LqFilter::MetaOwner { .. }
                | LqFilter::MetaService { .. }
                | LqFilter::MetaLayer { .. }
                | LqFilter::MetaSurface { .. }
                | LqFilter::Affected { .. }
                | LqFilter::InvalidatedBy { .. }
                | LqFilter::Fork { .. }
                | LqFilter::Archived { .. }
                | LqFilter::Visibility { .. }
                | LqFilter::Context { .. } => Err(CoreError::NotImplemented(
                    "history: one or more filters are not executable on the current adapter set"
                        .to_string(),
                )),
            },
            Self::RuntimeMetadata => match filter {
                LqFilter::Changed { scope } => {
                    state.saw_runtime_authority_filter = true;
                    let _: u64 = parse_runtime_changed_scope_ms(scope)?;
                    Ok(())
                }
                LqFilter::Stale { scope } => {
                    state.saw_runtime_authority_filter = true;
                    let _: u64 = parse_runtime_stale_scope_ms(scope)?;
                    Ok(())
                }
                // `Dirty` carries a yes/no/only mode but, like the snapshot /
                // meta / edge authority filters, only needs to record that a
                // runtime-authority filter was seen at planning time.
                LqFilter::Dirty { .. }
                | LqFilter::Snapshot { .. }
                | LqFilter::MetaOwner { .. }
                | LqFilter::MetaService { .. }
                | LqFilter::MetaLayer { .. }
                | LqFilter::MetaSurface { .. }
                | LqFilter::Affected { .. }
                | LqFilter::InvalidatedBy { .. } => {
                    state.saw_runtime_authority_filter = true;
                    Ok(())
                }
                LqFilter::File { .. } | LqFilter::Lang { .. } | LqFilter::Content { .. } => {
                    Ok(())
                }
                LqFilter::Repo { .. }
                | LqFilter::Rev { .. }
                | LqFilter::Author { .. }
                | LqFilter::Committer { .. }
                | LqFilter::Message { .. }
                | LqFilter::Before { .. }
                | LqFilter::After { .. }
                | LqFilter::Since { .. }
                | LqFilter::Until { .. }
                | LqFilter::DiffAdded { .. }
                | LqFilter::DiffRemoved { .. }
                | LqFilter::DiffTouched { .. }
                | LqFilter::Type { .. }
                | LqFilter::Select { .. }
                | LqFilter::Fork { .. }
                | LqFilter::Archived { .. }
                | LqFilter::Visibility { .. }
                | LqFilter::Context { .. } => Err(CoreError::NotImplemented(
                    "runtime metadata: one or more filters are not executable on the current adapter set"
                        .to_string(),
                )),
            },
        }
    }

    fn finalize(self, state: ExecutableTextPlaneValidationState) -> Result<(), CoreError> {
        match self {
            Self::History => Ok(()),
            Self::RuntimeMetadata => {
                if !state.saw_runtime_authority_filter {
                    return Err(CoreError::InvalidContract(
                        "runtime metadata: at least one runtime authority filter is required (dirty/changed/stale/snapshot/meta.*/affected/invalidated_by)"
                            .to_string(),
                    ));
                }
                Ok(())
            }
        }
    }
}

fn validate_executable_text_query(
    query: &LqQuery,
    policy: ExecutableTextPlanePolicy,
) -> Result<(), CoreError> {
    if query.options.timeout_ms.is_some() {
        return Err(CoreError::InvalidContract(format!(
            "{}: timeout option is not executable on the current adapter set",
            policy.plane_name()
        )));
    }
    let mut state = ExecutableTextPlaneValidationState::default();
    for filter in &query.filters {
        policy.validate_filter(filter, &mut state)?;
    }
    policy.finalize(state)?;
    validate_executable_text_surface(&query.expr, policy)?;
    for filter in &query.filters {
        if let LqFilter::Content { leaf } = filter {
            validate_leaf_surface(leaf, policy)?;
        }
    }
    Ok(())
}

fn validate_history_query(query: &LqQuery) -> Result<(), CoreError> {
    validate_executable_text_query(query, ExecutableTextPlanePolicy::History)?;
    validate_history_timeref_filters(query)?;
    let _: HistoryQueryKind = resolve_history_query_kind(query)?;
    Ok(())
}

fn validate_history_timeref_filters(query: &LqQuery) -> Result<(), CoreError> {
    for filter in &query.filters {
        match filter {
            LqFilter::Since { timeref } => validate_history_since_timeref(timeref)?,
            LqFilter::Before { timeref }
            | LqFilter::After { timeref }
            | LqFilter::Until { timeref } => {
                let _: u64 = parse_history_timeref_ms(timeref)?;
            }
            LqFilter::Repo { .. }
            | LqFilter::File { .. }
            | LqFilter::Lang { .. }
            | LqFilter::Rev { .. }
            | LqFilter::Author { .. }
            | LqFilter::Committer { .. }
            | LqFilter::Message { .. }
            | LqFilter::Type { .. }
            | LqFilter::Select { .. }
            | LqFilter::Dirty { .. }
            | LqFilter::Changed { .. }
            | LqFilter::Stale { .. }
            | LqFilter::Snapshot { .. }
            | LqFilter::MetaOwner { .. }
            | LqFilter::MetaService { .. }
            | LqFilter::MetaLayer { .. }
            | LqFilter::MetaSurface { .. }
            | LqFilter::Affected { .. }
            | LqFilter::InvalidatedBy { .. }
            | LqFilter::Fork { .. }
            | LqFilter::Archived { .. }
            | LqFilter::Visibility { .. }
            | LqFilter::Context { .. }
            | LqFilter::Content { .. }
            | LqFilter::DiffAdded { .. }
            | LqFilter::DiffRemoved { .. }
            | LqFilter::DiffTouched { .. } => {}
        }
    }
    Ok(())
}

#[expect(
    clippy::struct_excessive_bools,
    reason = "history shard readiness is modeled as four independent materialization bits"
)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct HistoryShardRequirements {
    commits: bool,
    refs: bool,
    tags: bool,
    diff_hunks: bool,
}

/// The history snapshot a query scans, taken under the ledger lock and
/// scanned after it is released.
fn resolve_history_state(
    ledger: &Ledger,
    pin: &GenerationPin,
    query: &LqQuery,
) -> Result<Arc<HistoryAuthorityState>, CoreError> {
    let Some(history_state) =
        ledger.history_snapshot(&pin.repo_id, &pin.revision_id, pin.manifest_generation)
    else {
        let lexical_materialized = ledger.track_materialized(
            &pin.repo_id,
            &pin.revision_id,
            SearchPlaneTrackKind::Lexical,
        );
        return Err(history_absent_error(pin, lexical_materialized));
    };
    ensure_history_shards_ready(&history_state, query)?;
    Ok(history_state)
}

fn history_absent_error(
    pin: &GenerationPin,
    lexical_materialized: Option<ManifestGeneration>,
) -> CoreError {
    match lexical_materialized {
        Some(materialized) if materialized.get() >= pin.manifest_generation.get() => {
            CoreError::Typed {
                code: ERR_HISTORY_PRODUCER_UNAVAILABLE.to_string(),
                message: format!(
                    "history: producer data is unavailable for generation {}",
                    pin.manifest_generation.get()
                ),
            }
        }
        _ => CoreError::Typed {
            code: ERR_HISTORY_GENERATION_NOT_READY.to_string(),
            message: format!(
                "history: generation {} is not yet materialized",
                pin.manifest_generation.get()
            ),
        },
    }
}

fn ensure_history_shards_ready(
    state: &HistoryAuthorityState,
    query: &LqQuery,
) -> Result<(), CoreError> {
    let requirements = history_shard_requirements(query)?;
    if requirements.commits && !state.commits_materialized() {
        return Err(history_shard_unavailable(
            "history: commit shard is unavailable for the requested query",
        ));
    }
    if requirements.refs && !state.refs_materialized() {
        return Err(history_shard_unavailable(
            "history: ref shard is unavailable for the requested query",
        ));
    }
    if requirements.tags && !state.tags_materialized() {
        return Err(history_shard_unavailable(
            "history: tag shard is unavailable for the requested query",
        ));
    }
    if requirements.diff_hunks && !state.diff_hunks_materialized() {
        return Err(history_shard_unavailable(
            "history: diff shard is unavailable for the requested query",
        ));
    }
    Ok(())
}

fn history_shard_requirements(query: &LqQuery) -> Result<HistoryShardRequirements, CoreError> {
    let mut requirements = match resolve_history_query_kind(query)? {
        HistoryQueryKind::Commit => HistoryShardRequirements {
            commits: true,
            ..HistoryShardRequirements::default()
        },
        HistoryQueryKind::Diff => HistoryShardRequirements {
            commits: true,
            diff_hunks: true,
            ..HistoryShardRequirements::default()
        },
    };
    for filter in &query.filters {
        if let LqFilter::Rev { spec } = filter
            && CommitSha::from_hex(spec).is_err()
        {
            requirements.refs = true;
            requirements.tags = true;
        }
    }
    Ok(requirements)
}

fn history_shard_unavailable(message: impl Into<String>) -> CoreError {
    CoreError::Typed {
        code: ERR_HISTORY_SHARD_UNAVAILABLE.to_string(),
        message: message.into(),
    }
}

fn history_invalid_request(message: impl Into<String>) -> CoreError {
    CoreError::InvalidContract(message.into())
}

fn validate_runtime_metadata_query(query: &LqQuery) -> Result<(), CoreError> {
    validate_executable_text_query(query, ExecutableTextPlanePolicy::RuntimeMetadata)?;
    if runtime_query_requires_catalog(query) {
        // Scope parsing for changed/stale is validated in validate_filter; this
        // pass is reserved for future cross-filter catalog constraints.
    }
    Ok(())
}

fn runtime_query_requires_catalog(query: &LqQuery) -> bool {
    query.filters.iter().any(|filter| {
        matches!(
            filter,
            LqFilter::Changed { .. }
                | LqFilter::Stale { .. }
                | LqFilter::Snapshot { .. }
                | LqFilter::MetaOwner { .. }
                | LqFilter::MetaService { .. }
                | LqFilter::MetaLayer { .. }
                | LqFilter::MetaSurface { .. }
                | LqFilter::Affected { .. }
                | LqFilter::InvalidatedBy { .. }
        )
    })
}

fn ensure_runtime_catalog_ready(state: &RuntimeMetadataState) -> Result<(), CoreError> {
    if state.catalog_materialized() {
        Ok(())
    } else {
        Err(CoreError::Typed {
            code: ERR_RUNTIME_CATALOG_NOT_READY.to_string(),
            message: "runtime metadata: catalog is not materialized for the pinned generation"
                .to_string(),
        })
    }
}

fn runtime_invalid_scope(message: impl Into<String>) -> CoreError {
    CoreError::Typed {
        code: ERR_RUNTIME_INVALID_SCOPE.to_string(),
        message: message.into(),
    }
}

fn runtime_catalog_head_missing(field: &str) -> CoreError {
    CoreError::Typed {
        code: ERR_RUNTIME_CATALOG_HEAD_MISSING.to_string(),
        message: format!("runtime metadata: catalog field `{field}` is not materialized"),
    }
}

fn runtime_snapshot_unknown(name: &str) -> CoreError {
    CoreError::Typed {
        code: ERR_SNAPSHOT_UNKNOWN.to_string(),
        message: format!("runtime metadata: snapshot `{name}` is unknown in the pinned catalog"),
    }
}

fn ensure_runtime_snapshot_names_known(
    query: &LqQuery,
    state: &RuntimeMetadataState,
) -> Result<(), CoreError> {
    for filter in &query.filters {
        if let LqFilter::Snapshot { name } = filter
            && !state.snapshots().contains_key(name.as_str())
        {
            return Err(runtime_snapshot_unknown(name));
        }
    }
    Ok(())
}

fn parse_runtime_changed_scope_ms(scope: &str) -> Result<u64, CoreError> {
    let Some(timeref) = scope.strip_prefix("since=") else {
        return Err(runtime_invalid_scope(format!(
            "runtime metadata: changed scope `{scope}` must use since=<timeref>"
        )));
    };
    parse_history_timeref_ms(timeref).map_err(|err| {
        runtime_invalid_scope(format!(
            "runtime metadata: changed scope timeref `{timeref}` is not a valid RFC3339 timestamp or duration: {err}"
        ))
    })
}

fn parse_runtime_stale_scope_ms(scope: &str) -> Result<u64, CoreError> {
    let Some(timeref) = scope.strip_prefix("before=") else {
        return Err(runtime_invalid_scope(format!(
            "runtime metadata: stale scope `{scope}` must use before=<timeref>"
        )));
    };
    parse_history_timeref_ms(timeref).map_err(|err| {
        runtime_invalid_scope(format!(
            "runtime metadata: stale scope timeref `{timeref}` is not a valid RFC3339 timestamp or duration: {err}"
        ))
    })
}

fn lower_structural_query_request(
    activation_catalog: &ActivationCatalog,
    request: &StructuralQueryRequest,
) -> Result<(GenerationPin, LqQuery), CoreError> {
    let lowered = match request.text_query.syntax {
        TextQuerySyntax::Native => lower_lexical_text_query(&request.text_query)?,
        TextQuerySyntax::Sourcegraph => {
            lower_sourcegraph_structural_query_text(&request.text_query.query_text)?
        }
    };
    validate_structural_feature_surface(&lowered)?;
    let pin = resolve_lexical_request_pin(
        activation_catalog,
        &request.text_query,
        SearchPlaneTrackKind::Structural,
        "structural",
    )?;
    Ok((pin, lowered))
}

fn validate_structural_feature_surface(query: &LqQuery) -> Result<(), CoreError> {
    if query.options.timeout_ms.is_some() {
        return Err(structural_invalid_request(
            "structural: timeout option is not executable on the current authority route",
        ));
    }
    reject_typed_structural_holes_in_expr(&query.expr)
}

fn reject_typed_structural_holes_in_expr(expr: &LqExpr) -> Result<(), CoreError> {
    match expr {
        LqExpr::Empty
        | LqExpr::Leaf(
            LqLeaf::Keyword(_)
            | LqLeaf::Phrase(_)
            | LqLeaf::RawString(_)
            | LqLeaf::Regex(_)
            | LqLeaf::Predicate { .. },
        ) => Ok(()),
        LqExpr::Leaf(LqLeaf::StructuralBlock(block)) => {
            reject_typed_structural_holes_in_block(block)
        }
        LqExpr::Not(inner) => reject_typed_structural_holes_in_expr(inner),
        LqExpr::All(children) | LqExpr::Any(children) => {
            for child in children {
                reject_typed_structural_holes_in_expr(child)?;
            }
            Ok(())
        }
    }
}

fn reject_typed_structural_holes_in_block(block: &LqStructuralBlock) -> Result<(), CoreError> {
    for expr in &block.exprs {
        reject_typed_structural_holes_in_structural_expr(expr)?;
    }
    Ok(())
}

fn reject_typed_structural_holes_in_structural_expr(
    expr: &LqStructuralExpr,
) -> Result<(), CoreError> {
    match expr {
        LqStructuralExpr::Pattern(nodes) => {
            for node in nodes {
                reject_typed_structural_holes_in_node(node)?;
            }
            Ok(())
        }
        LqStructuralExpr::Where(constraints) => {
            for constraint in constraints {
                reject_typed_structural_holes_in_constraint(constraint)?;
            }
            Ok(())
        }
        LqStructuralExpr::Inside(block) | LqStructuralExpr::Outside(block) => {
            reject_typed_structural_holes_in_block(block)
        }
    }
}

fn reject_typed_structural_holes_in_node(node: &LqStructuralNode) -> Result<(), CoreError> {
    match node {
        LqStructuralNode::Literal(_)
        | LqStructuralNode::MetaVar(_)
        | LqStructuralNode::WildcardMany => Ok(()),
        LqStructuralNode::Group(children) => {
            for child in children {
                reject_typed_structural_holes_in_node(child)?;
            }
            Ok(())
        }
        LqStructuralNode::Hole { name, multiplicity } => reject_typed_structural_hole_name(
            name.as_ref().map(quanta_index_contract::LqMetaVar::as_str),
            *multiplicity,
        ),
    }
}

fn reject_typed_structural_holes_in_constraint(
    constraint: &LqStructuralConstraint,
) -> Result<(), CoreError> {
    reject_typed_structural_hole_ref(&constraint.left)?;
    if let LqStructuralConstraintOperand::Hole(hole) = &constraint.right {
        reject_typed_structural_hole_ref(hole)?;
    }
    Ok(())
}

fn reject_typed_structural_hole_ref(hole: &LqStructuralHoleRef) -> Result<(), CoreError> {
    reject_typed_structural_hole_name(Some(hole.name.as_str()), hole.multiplicity)
}

fn reject_typed_structural_hole_name(
    name: Option<&str>,
    multiplicity: quanta_index_contract::LqStructuralHoleMultiplicity,
) -> Result<(), CoreError> {
    let Some(name) = name else {
        return Ok(());
    };
    let Some((metavar, kind)) = name.rsplit_once('.') else {
        return Ok(());
    };
    if metavar.is_empty() || kind.is_empty() {
        return Ok(());
    }
    if multiplicity != quanta_index_contract::LqStructuralHoleMultiplicity::One {
        return Err(CoreError::Typed {
            code: LexicalErrorCode::StrHoleKindUnsupported
                .as_code_str()
                .to_string(),
            message: format!(
                "structural typed hole kind `{kind}` is executable only on single-capture holes"
            ),
        });
    }
    if matches!(kind, "expr" | "stmt" | "item" | "type") {
        return Ok(());
    }
    Err(CoreError::Typed {
        code: LexicalErrorCode::StrHoleKindUnsupported
            .as_code_str()
            .to_string(),
        message: format!(
            "structural typed hole kind `{kind}` is not executable on the current authority route"
        ),
    })
}

type StructuralCandidateBuckets = BTreeMap<String, Vec<StructuralMatchCandidate>>;

struct LexicalSubexprEvaluator<'a> {
    dispatcher: &'a SearchPlaneDispatcher,
    pin: &'a GenerationPin,
    query: &'a LqQuery,
    budget: &'a RequestBudgetV1,
}

impl LexicalSubexprEvaluator<'_> {
    fn evaluate(&self, expr: &LqExpr) -> Result<StructuralCandidateBuckets, CoreError> {
        let mut options = self.query.options.clone();
        if options.pattern_type == LqPatternType::Structural {
            options.pattern_type = LqPatternType::Standard;
        }
        let subquery = LqQuery {
            lq_version: LQ_VERSION_TAG,
            expr: expr.clone(),
            filters: self.query.filters.clone(),
            directives: self.query.directives.clone(),
            options,
            source_span: self.query.source_span,
        };
        LexicalPolicy::validate_query(&subquery)?;
        let materialized = self
            .dispatcher
            .snapshot_lex_materialized(&self.pin.repo_id, &self.pin.revision_id)?;
        LexicalPolicy::validate_query_against_readiness(
            self.pin.manifest_generation,
            materialized,
        )?;
        let searcher = self.dispatcher.acquire_lexical(
            &self.pin.repo_id,
            &self.pin.revision_id,
            self.pin.manifest_generation,
        )?;
        // Every lexical leaf of a structural expression is its own native
        // search; a boolean tree can hold many, so each one is a checkpoint.
        self.budget.checkpoint("structural:lexical-leaf")?;
        if symbol_name_predicate_leaf(expr) {
            let results = searcher.search_symbols_all(&subquery)?;
            let structural_state = self.dispatcher.snapshot_structural_state(self.pin)?;
            return Ok(symbol_hits_to_structural_buckets(
                results,
                &structural_state,
            ));
        }
        let results = searcher.search_all(&subquery)?;
        Ok(lexical_hits_to_structural_buckets(results))
    }
}

fn symbol_name_predicate_leaf(expr: &LqExpr) -> bool {
    matches!(
        expr,
        LqExpr::Leaf(LqLeaf::Predicate { name, .. }) if name == "symbol.has.name"
    )
}

fn lexical_hits_to_structural_buckets(
    results: Vec<LexicalCandidate>,
) -> StructuralCandidateBuckets {
    let mut buckets = StructuralCandidateBuckets::new();
    for hit in results {
        buckets
            .entry(hit.candidate_id.clone())
            .or_default()
            .push(StructuralMatchCandidate {
                candidate_id: hit.candidate_id,
                pattern_start_byte: 0,
                pattern_end_byte: 0,
                bindings: Vec::new(),
            });
    }
    for bucket in buckets.values_mut() {
        normalize_structural_match_bucket(bucket);
    }
    buckets
}

fn symbol_hits_to_structural_buckets(
    results: Vec<SymbolCandidate>,
    structural_state: &StructuralAuthorityState,
) -> StructuralCandidateBuckets {
    let mut buckets = StructuralCandidateBuckets::new();
    for hit in results {
        for (chunk_id, chunk) in structural_state.chunks() {
            if chunk.repo_relative_path != hit.repo_relative_path {
                continue;
            }
            if !symbol_span_overlaps_chunk_lines(&hit, chunk) {
                continue;
            }
            let candidate_id = chunk_id.as_str().to_string();
            buckets
                .entry(candidate_id.clone())
                .or_default()
                .push(StructuralMatchCandidate {
                    candidate_id,
                    pattern_start_byte: chunk.start_byte,
                    pattern_end_byte: chunk.end_byte,
                    bindings: Vec::new(),
                });
        }
    }
    for bucket in buckets.values_mut() {
        normalize_structural_match_bucket(bucket);
    }
    buckets
}

fn symbol_span_overlaps_chunk_lines(hit: &SymbolCandidate, chunk: &ChunkRecord) -> bool {
    let hit_start = hit.start_line.max(1);
    let hit_end = hit.end_line.max(hit_start);
    let chunk_start = chunk.start_line.max(1);
    let chunk_end = chunk.end_line.max(chunk_start);
    hit_start <= chunk_end && hit_end >= chunk_start
}

fn compile_structural_filter_regex(
    filter_name: &str,
    pattern: &str,
) -> Result<RegexExecutor, CoreError> {
    RegexExecutor::compile(pattern).map_err(|err| {
        structural_invalid_request(format!(
            "{filter_name} filter pattern failed to compile as regex: {err}"
        ))
    })
}

fn repo_matches_structural_filters(
    pin: &GenerationPin,
    filters: &[StructuralExecutableFilter],
) -> Result<bool, CoreError> {
    for filter in filters {
        if let StructuralExecutableFilter::RepoRegexNoRev { pattern } = filter {
            let executor = compile_structural_filter_regex("repo", pattern)?;
            if !executor.verify(pin.repo_id.as_str().as_bytes()) {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

fn chunk_matches_structural_filters(
    chunk: &ChunkRecord,
    filters: &[StructuralExecutableFilter],
) -> Result<bool, CoreError> {
    for filter in filters {
        match filter {
            StructuralExecutableFilter::RepoRegexNoRev { .. } => {}
            StructuralExecutableFilter::FileRegex { pattern, scope } => {
                let executor = compile_structural_filter_regex("file", pattern)?;
                let path = chunk.repo_relative_path.as_str();
                let path_match = executor.verify(path.as_bytes());
                let matched = match scope {
                    LqFileScope::PathOnly => path_match,
                    LqFileScope::NameOnly => path
                        .rsplit('/')
                        .next()
                        .is_some_and(|name| executor.verify(name.as_bytes())),
                    LqFileScope::NameAndPath => {
                        path_match
                            || path
                                .rsplit('/')
                                .next()
                                .is_some_and(|name| executor.verify(name.as_bytes()))
                    }
                };
                if !matched {
                    return Ok(false);
                }
            }
        }
    }
    Ok(true)
}

fn build_pinned_structural_universe(
    pin: &GenerationPin,
    structural_state: &StructuralAuthorityState,
    requested_lang: Option<&str>,
    filters: &[StructuralExecutableFilter],
) -> Result<StructuralCandidateBuckets, CoreError> {
    if !repo_matches_structural_filters(pin, filters)? {
        return Ok(StructuralCandidateBuckets::new());
    }
    let mut buckets = StructuralCandidateBuckets::new();
    for (chunk_id, chunk) in structural_state.chunks() {
        if let Some(lang) = requested_lang
            && chunk.language.as_str() != lang
        {
            continue;
        }
        if !chunk_matches_structural_filters(chunk, filters)? {
            continue;
        }
        let candidate_id = chunk_id.as_str().to_string();
        buckets
            .entry(candidate_id.clone())
            .or_default()
            .push(StructuralMatchCandidate {
                candidate_id,
                pattern_start_byte: chunk.start_byte,
                pattern_end_byte: chunk.end_byte,
                bindings: Vec::new(),
            });
    }
    for bucket in buckets.values_mut() {
        normalize_structural_match_bucket(bucket);
    }
    Ok(buckets)
}

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
struct StructuralLeafExecutionKey {
    pattern: LqStructuralBlock,
    requested_lang: Option<String>,
    filters: Vec<StructuralExecutableFilter>,
    candidate_scope: Option<Vec<String>>,
    options: LqOptions,
}

#[expect(
    clippy::disallowed_types,
    reason = "internal structural leaf memo cache is transient and not part of any persisted or external ordering surface"
)]
type StructuralLeafCache =
    std::collections::HashMap<StructuralLeafExecutionKey, StructuralCandidateBuckets>;

#[derive(Default)]
struct StructuralEvalContext {
    leaf_cache: StructuralLeafCache,
}

fn structural_expr_has_structural_leaf(expr: &LqExpr) -> bool {
    match expr {
        LqExpr::Leaf(LqLeaf::StructuralBlock(_)) => true,
        LqExpr::Empty | LqExpr::Leaf(_) => false,
        LqExpr::Not(inner) => structural_expr_has_structural_leaf(inner),
        LqExpr::All(children) | LqExpr::Any(children) => {
            children.iter().any(structural_expr_has_structural_leaf)
        }
    }
}

fn structural_expr_has_non_structural_leaf(expr: &LqExpr) -> bool {
    match expr {
        LqExpr::Empty | LqExpr::Leaf(LqLeaf::StructuralBlock(_)) => false,
        LqExpr::Leaf(_) => true,
        LqExpr::Not(inner) => structural_expr_has_non_structural_leaf(inner),
        LqExpr::All(children) | LqExpr::Any(children) => {
            children.iter().any(structural_expr_has_non_structural_leaf)
        }
    }
}

fn structural_expr_is_pure_negative_root(expr: &LqExpr) -> bool {
    matches!(expr, LqExpr::Not(_))
}

fn extract_structural_requested_lang(
    expr: &LqExpr,
    allow_lexical_leaves: bool,
) -> Result<Option<String>, CoreError> {
    let mut requested_lang = None;
    collect_structural_requested_lang(expr, &mut requested_lang, allow_lexical_leaves)?;
    Ok(requested_lang)
}

fn collect_structural_requested_lang(
    expr: &LqExpr,
    requested_lang: &mut Option<String>,
    allow_lexical_leaves: bool,
) -> Result<(), CoreError> {
    match expr {
        LqExpr::Empty => Err(structural_invalid_request(
            "query must include at least one structural `match { ... }` leaf",
        )),
        LqExpr::Leaf(LqLeaf::StructuralBlock(block)) => {
            merge_structural_requested_lang(requested_lang, block.lang.as_deref())
        }
        LqExpr::Leaf(_) => {
            if allow_lexical_leaves {
                Ok(())
            } else {
                Err(structural_invalid_request(
                    "query must lower to a structural-only boolean tree of `match { ... }` leaves",
                ))
            }
        }
        LqExpr::Not(inner) => {
            collect_structural_requested_lang(inner, requested_lang, allow_lexical_leaves)
        }
        LqExpr::All(children) | LqExpr::Any(children) => {
            if children.is_empty() {
                return Err(structural_invalid_request(
                    "query must include at least one structural `match { ... }` leaf",
                ));
            }
            for child in children {
                collect_structural_requested_lang(child, requested_lang, allow_lexical_leaves)?;
            }
            Ok(())
        }
    }
}

fn merge_structural_requested_lang(
    requested_lang: &mut Option<String>,
    candidate_lang: Option<&str>,
) -> Result<(), CoreError> {
    let candidate_lang = candidate_lang
        .map(str::trim)
        .filter(|lang| !lang.is_empty())
        .map(str::to_string);
    match (requested_lang.as_deref(), candidate_lang.as_deref()) {
        (_, None) => Ok(()),
        (None, Some(lang)) => {
            *requested_lang = Some(lang.to_string());
            Ok(())
        }
        (Some(current), Some(lang)) if current == lang => Ok(()),
        (Some(current), Some(lang)) => Err(structural_invalid_request(format!(
            "conflicting structural lang requirements `{current}` and `{lang}` are not allowed"
        ))),
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "structural dispatch context; bundling is a separate refactor"
)]
fn evaluate_structural_expr(
    ctx: &mut StructuralEvalContext,
    service: &StructuralService,
    pin: &GenerationPin,
    expr: &LqExpr,
    requested_lang: Option<&str>,
    filters: &[StructuralExecutableFilter],
    options: &LqOptions,
    seed: Option<&StructuralCandidateBuckets>,
    lexical_eval: Option<&LexicalSubexprEvaluator<'_>>,
) -> Result<StructuralCandidateBuckets, CoreError> {
    match expr {
        LqExpr::Empty => Err(structural_invalid_request(
            "query must include at least one structural `match { ... }` leaf",
        )),
        LqExpr::Leaf(LqLeaf::StructuralBlock(block)) => execute_structural_block(
            ctx,
            service,
            pin,
            block,
            requested_lang,
            filters,
            options,
            seed,
        ),
        LqExpr::Leaf(_) => {
            let Some(evaluator) = lexical_eval else {
                return Err(structural_invalid_request(
                    "query must lower to a structural-only boolean tree of `match { ... }` leaves",
                ));
            };
            evaluator.evaluate(expr)
        }
        LqExpr::Not(inner) => {
            let Some(seed) = seed else {
                return Err(structural_invalid_request(
                    "pure-negative structural boolean queries are not executable; add a positive structural leaf before `NOT`",
                ));
            };
            let blocked = evaluate_structural_expr(
                ctx,
                service,
                pin,
                inner,
                requested_lang,
                filters,
                options,
                Some(seed),
                lexical_eval,
            )?;
            Ok(subtract_structural_buckets(seed, &blocked))
        }
        LqExpr::All(children) => {
            if children.is_empty() {
                return Err(structural_invalid_request(
                    "query must include at least one structural `match { ... }` leaf",
                ));
            }
            let mut positives = children
                .iter()
                .filter(|child| !matches!(child, LqExpr::Not(_)));
            let mut current = if let Some(first_positive) = positives.next() {
                let mut current = evaluate_structural_expr(
                    ctx,
                    service,
                    pin,
                    first_positive,
                    requested_lang,
                    filters,
                    options,
                    seed,
                    lexical_eval,
                )?;
                for child in positives {
                    let next = evaluate_structural_expr(
                        ctx,
                        service,
                        pin,
                        child,
                        requested_lang,
                        filters,
                        options,
                        Some(&current),
                        lexical_eval,
                    )?;
                    current = intersect_structural_buckets(&current, &next);
                    if current.is_empty() {
                        return Ok(current);
                    }
                }
                current
            } else if let Some(seed) = seed {
                seed.clone()
            } else {
                return Err(structural_invalid_request(
                    "pure-negative structural boolean queries are not executable; add a positive structural leaf before `NOT`",
                ));
            };
            for child in children {
                if matches!(child, LqExpr::Not(_)) {
                    current = evaluate_structural_expr(
                        ctx,
                        service,
                        pin,
                        child,
                        requested_lang,
                        filters,
                        options,
                        Some(&current),
                        lexical_eval,
                    )?;
                    if current.is_empty() {
                        return Ok(current);
                    }
                }
            }
            Ok(current)
        }
        LqExpr::Any(children) => {
            if children.is_empty() {
                return Err(structural_invalid_request(
                    "query must include at least one structural `match { ... }` leaf",
                ));
            }
            let mut union = StructuralCandidateBuckets::new();
            for child in children {
                let child_matches = evaluate_structural_expr(
                    ctx,
                    service,
                    pin,
                    child,
                    requested_lang,
                    filters,
                    options,
                    seed,
                    lexical_eval,
                )?;
                union = union_structural_buckets(union, child_matches);
            }
            Ok(union)
        }
    }
}

fn execute_structural_block(
    ctx: &mut StructuralEvalContext,
    service: &StructuralService,
    pin: &GenerationPin,
    block: &LqStructuralBlock,
    requested_lang: Option<&str>,
    filters: &[StructuralExecutableFilter],
    options: &LqOptions,
    seed: Option<&StructuralCandidateBuckets>,
) -> Result<StructuralCandidateBuckets, CoreError> {
    let candidate_scope = seed.map(structural_candidate_scope_ids);
    if candidate_scope.as_ref().is_some_and(Vec::is_empty) {
        return Ok(StructuralCandidateBuckets::new());
    }
    let cache_key = StructuralLeafExecutionKey {
        pattern: block.clone(),
        requested_lang: requested_lang.map(str::to_string),
        filters: filters.to_vec(),
        candidate_scope: candidate_scope.clone(),
        options: options.clone(),
    };
    if let Some(cached) = ctx.leaf_cache.get(&cache_key) {
        return Ok(cached.clone());
    }
    let response = service
        .query(&DomainStructuralQueryRequest {
            pattern: block.clone(),
            requested_lang: requested_lang.map(str::to_string),
            filters: filters.to_vec(),
            candidate_scope,
            options: options.clone(),
            generation: GenerationSelector::Pinned(pin.clone()),
        })
        .map_err(|err| map_structural_error(&err))?;
    let buckets = bucket_structural_matches(response.candidates);
    let _prior = ctx.leaf_cache.insert(cache_key, buckets.clone());
    Ok(buckets)
}

fn structural_candidate_scope_ids(candidates: &StructuralCandidateBuckets) -> Vec<String> {
    candidates.keys().cloned().collect()
}

fn bucket_structural_matches(
    candidates: Vec<StructuralMatchCandidate>,
) -> StructuralCandidateBuckets {
    let mut buckets = StructuralCandidateBuckets::new();
    for mut candidate in candidates {
        normalize_structural_match_candidate(&mut candidate);
        buckets
            .entry(candidate.candidate_id.clone())
            .or_default()
            .push(candidate);
    }
    for bucket in buckets.values_mut() {
        normalize_structural_match_bucket(bucket);
    }
    buckets
}

fn union_structural_buckets(
    mut left: StructuralCandidateBuckets,
    right: StructuralCandidateBuckets,
) -> StructuralCandidateBuckets {
    for (candidate_id, mut matches) in right {
        left.entry(candidate_id).or_default().append(&mut matches);
    }
    for bucket in left.values_mut() {
        normalize_structural_match_bucket(bucket);
    }
    left
}

fn intersect_structural_buckets(
    left: &StructuralCandidateBuckets,
    right: &StructuralCandidateBuckets,
) -> StructuralCandidateBuckets {
    let mut merged = StructuralCandidateBuckets::new();
    for (candidate_id, left_matches) in left {
        let Some(right_matches) = right.get(candidate_id) else {
            continue;
        };
        let combined = if left_matches
            .iter()
            .all(|candidate| candidate.bindings.is_empty())
        {
            vec![merge_structural_identity_matches(
                candidate_id,
                right_matches,
            )]
        } else {
            merge_structural_match_sets(candidate_id, left_matches, right_matches)
        };
        if !combined.is_empty() {
            let _prior = merged.insert(candidate_id.clone(), combined);
        }
    }
    merged
}

fn subtract_structural_buckets(
    base: &StructuralCandidateBuckets,
    blocked: &StructuralCandidateBuckets,
) -> StructuralCandidateBuckets {
    let mut remaining = StructuralCandidateBuckets::new();
    for (candidate_id, base_matches) in base {
        let next_matches = blocked.get(candidate_id).map_or_else(
            || base_matches.clone(),
            |blocked_matches| {
                base_matches
                    .iter()
                    .filter(|candidate| {
                        !blocked_matches.iter().any(|blocked_candidate| {
                            structural_match_candidates_consistent(candidate, blocked_candidate)
                        })
                    })
                    .cloned()
                    .collect::<Vec<_>>()
            },
        );
        if !next_matches.is_empty() {
            let _prior = remaining.insert(candidate_id.clone(), next_matches);
        }
    }
    remaining
}

fn merge_structural_identity_matches(
    candidate_id: &str,
    matches: &[StructuralMatchCandidate],
) -> StructuralMatchCandidate {
    let mut bindings = Vec::new();
    let mut pattern_start_byte = matches
        .first()
        .map_or(0, |candidate| candidate.pattern_start_byte);
    let mut pattern_end_byte = matches
        .first()
        .map_or(0, |candidate| candidate.pattern_end_byte);
    for candidate in matches {
        pattern_start_byte = pattern_start_byte.min(candidate.pattern_start_byte);
        pattern_end_byte = pattern_end_byte.min(candidate.pattern_end_byte);
        for binding in &candidate.bindings {
            if !bindings.iter().any(|existing| existing == binding) {
                bindings.push(binding.clone());
            }
        }
    }
    bindings.sort_by(compare_structural_bindings);
    StructuralMatchCandidate {
        candidate_id: candidate_id.to_string(),
        pattern_start_byte,
        pattern_end_byte,
        bindings,
    }
}

fn merge_structural_match_sets(
    candidate_id: &str,
    left: &[StructuralMatchCandidate],
    right: &[StructuralMatchCandidate],
) -> Vec<StructuralMatchCandidate> {
    let mut merged = Vec::new();
    for left_candidate in left {
        for right_candidate in right {
            if !structural_match_candidates_consistent(left_candidate, right_candidate) {
                continue;
            }
            let mut bindings = left_candidate.bindings.clone();
            for binding in &right_candidate.bindings {
                if !bindings.iter().any(|existing| existing == binding) {
                    bindings.push(binding.clone());
                }
            }
            bindings.sort_by(compare_structural_bindings);
            let (pattern_start_byte, pattern_end_byte) = std::cmp::min(
                (
                    left_candidate.pattern_start_byte,
                    left_candidate.pattern_end_byte,
                ),
                (
                    right_candidate.pattern_start_byte,
                    right_candidate.pattern_end_byte,
                ),
            );
            merged.push(StructuralMatchCandidate {
                candidate_id: candidate_id.to_string(),
                pattern_start_byte,
                pattern_end_byte,
                bindings,
            });
        }
    }
    normalize_structural_match_bucket(&mut merged);
    merged
}

fn structural_match_candidates_consistent(
    left: &StructuralMatchCandidate,
    right: &StructuralMatchCandidate,
) -> bool {
    left.bindings.iter().all(|left_binding| {
        right.bindings.iter().all(|right_binding| {
            left_binding.metavariable != right_binding.metavariable
                || compare_structural_bindings(left_binding, right_binding).is_eq()
        })
    })
}

fn normalize_structural_match_bucket(bucket: &mut Vec<StructuralMatchCandidate>) {
    for candidate in bucket.iter_mut() {
        normalize_structural_match_candidate(candidate);
    }
    bucket.sort_by(compare_structural_match_candidates);
    bucket.dedup_by(|left, right| {
        left.candidate_id == right.candidate_id
            && left.pattern_start_byte == right.pattern_start_byte
            && left.pattern_end_byte == right.pattern_end_byte
            && left.bindings == right.bindings
    });
}

fn normalize_structural_match_candidate(candidate: &mut StructuralMatchCandidate) {
    candidate.bindings.sort_by(compare_structural_bindings);
    candidate.bindings.dedup_by(|left, right| left == right);
}

fn extract_structural_filters(
    query: &LqQuery,
    initial_lang: Option<&str>,
) -> Result<(Option<String>, Vec<StructuralExecutableFilter>), CoreError> {
    let mut requested_lang: Option<String> = initial_lang.map(str::to_string);
    let mut executable_filters: Vec<StructuralExecutableFilter> = Vec::new();
    for filter in &query.filters {
        match filter {
            LqFilter::Lang { id } => match requested_lang.as_deref() {
                None => requested_lang = Some(id.clone()),
                Some(current) if current == id => {}
                Some(current) => {
                    return Err(structural_invalid_request(format!(
                        "conflicting lang filters `{current}` and `{id}` are not allowed"
                    )));
                }
            },
            LqFilter::Repo { pattern, revs } => {
                if !revs.is_empty() {
                    return Err(structural_invalid_request(
                        "repo filter revisions are not executable on the current structural adapter set",
                    ));
                }
                executable_filters.push(StructuralExecutableFilter::RepoRegexNoRev {
                    pattern: pattern.clone(),
                });
            }
            LqFilter::File { pattern, scope } => {
                executable_filters.push(StructuralExecutableFilter::FileRegex {
                    pattern: pattern.clone(),
                    scope: *scope,
                });
            }
            other @ (LqFilter::Rev { .. }
            | LqFilter::Author { .. }
            | LqFilter::Committer { .. }
            | LqFilter::Message { .. }
            | LqFilter::Before { .. }
            | LqFilter::After { .. }
            | LqFilter::Since { .. }
            | LqFilter::Until { .. }
            | LqFilter::DiffAdded { .. }
            | LqFilter::DiffRemoved { .. }
            | LqFilter::DiffTouched { .. }
            | LqFilter::Type { .. }
            | LqFilter::Select { .. }
            | LqFilter::Dirty { .. }
            | LqFilter::Changed { .. }
            | LqFilter::Stale { .. }
            | LqFilter::Snapshot { .. }
            | LqFilter::MetaOwner { .. }
            | LqFilter::MetaService { .. }
            | LqFilter::MetaLayer { .. }
            | LqFilter::MetaSurface { .. }
            | LqFilter::Affected { .. }
            | LqFilter::InvalidatedBy { .. }
            | LqFilter::Fork { .. }
            | LqFilter::Archived { .. }
            | LqFilter::Content { .. }
            | LqFilter::Visibility { .. }
            | LqFilter::Context { .. }) => {
                return Err(structural_invalid_request(format!(
                    "filter `{}` is not executable on the current structural adapter set",
                    structural_filter_label(other)
                )));
            }
        }
    }
    Ok((requested_lang, executable_filters))
}

fn structural_filter_label(filter: &LqFilter) -> &'static str {
    match filter {
        LqFilter::Repo { .. } => "repo",
        LqFilter::File { .. } => "file",
        LqFilter::Lang { .. } => "lang",
        LqFilter::Rev { .. } => "rev",
        LqFilter::Author { .. } => "author",
        LqFilter::Committer { .. } => "committer",
        LqFilter::Message { .. } => "message",
        LqFilter::Before { .. } => "before",
        LqFilter::After { .. } => "after",
        LqFilter::Since { .. } => "since",
        LqFilter::Until { .. } => "until",
        LqFilter::DiffAdded { .. } => "diff.added",
        LqFilter::DiffRemoved { .. } => "diff.removed",
        LqFilter::DiffTouched { .. } => "diff.touched",
        LqFilter::Type { .. } => "type",
        LqFilter::Select { .. } => "select",
        LqFilter::Dirty { .. } => "dirty",
        LqFilter::Changed { .. } => "changed",
        LqFilter::Stale { .. } => "stale",
        LqFilter::Snapshot { .. } => "snapshot",
        LqFilter::MetaOwner { .. } => "meta.owner",
        LqFilter::MetaService { .. } => "meta.service",
        LqFilter::MetaLayer { .. } => "meta.layer",
        LqFilter::MetaSurface { .. } => "meta.surface",
        LqFilter::Affected { .. } => "affected",
        LqFilter::InvalidatedBy { .. } => "invalidated_by",
        LqFilter::Fork { .. } => "fork",
        LqFilter::Archived { .. } => "archived",
        LqFilter::Content { .. } => "content",
        LqFilter::Visibility { .. } => "visibility",
        LqFilter::Context { .. } => "context",
    }
}

fn structural_invalid_request(message: impl Into<String>) -> CoreError {
    CoreError::Typed {
        code: "STR_INVALID_REQUEST".to_string(),
        message: format!("structural: {}", message.into()),
    }
}

fn project_structural_query_results(
    candidates: StructuralCandidateBuckets,
) -> Vec<quanta_index_contract::StructuralCandidate> {
    candidates
        .into_iter()
        .filter_map(|(_, bucket)| {
            bucket
                .into_iter()
                .min_by(compare_structural_match_candidates)
        })
        .map(project_structural_query_candidate)
        .collect()
}

fn project_structural_query_candidate(
    candidate: StructuralMatchCandidate,
) -> quanta_index_contract::StructuralCandidate {
    quanta_index_contract::StructuralCandidate {
        candidate_id: candidate.candidate_id,
        bindings: candidate
            .bindings
            .into_iter()
            .map(project_structural_query_binding)
            .collect(),
    }
}

fn project_structural_query_binding(
    binding: StructuralMatchBinding,
) -> quanta_index_contract::StructuralBinding {
    quanta_index_contract::StructuralBinding {
        metavariable: binding.metavariable,
        start_byte: binding.start_byte,
        end_byte: binding.end_byte,
        start_line: binding.start_line,
        end_line: binding.end_line,
    }
}

fn compare_structural_match_candidates(
    left: &StructuralMatchCandidate,
    right: &StructuralMatchCandidate,
) -> std::cmp::Ordering {
    left.pattern_start_byte
        .cmp(&right.pattern_start_byte)
        .then_with(|| left.pattern_end_byte.cmp(&right.pattern_end_byte))
        .then_with(|| compare_structural_binding_lists(&left.bindings, &right.bindings))
        .then_with(|| left.candidate_id.as_str().cmp(right.candidate_id.as_str()))
}

fn compare_structural_binding_lists(
    left: &[StructuralMatchBinding],
    right: &[StructuralMatchBinding],
) -> std::cmp::Ordering {
    for (left_binding, right_binding) in left.iter().zip(right.iter()) {
        let ordering = compare_structural_bindings(left_binding, right_binding);
        if !ordering.is_eq() {
            return ordering;
        }
    }
    left.len().cmp(&right.len())
}

fn compare_structural_bindings(
    left: &StructuralMatchBinding,
    right: &StructuralMatchBinding,
) -> std::cmp::Ordering {
    left.metavariable
        .as_str()
        .cmp(right.metavariable.as_str())
        .then_with(|| left.start_byte.cmp(&right.start_byte))
        .then_with(|| left.end_byte.cmp(&right.end_byte))
        .then_with(|| left.start_line.cmp(&right.start_line))
        .then_with(|| left.end_line.cmp(&right.end_line))
}

fn map_structural_error(
    err: &quanta_index_core::domains::structural::StructuralError,
) -> CoreError {
    CoreError::Typed {
        code: err.code().to_string(),
        message: err.to_string(),
    }
}

fn validate_executable_text_surface(
    expr: &LqExpr,
    policy: ExecutableTextPlanePolicy,
) -> Result<(), CoreError> {
    match expr {
        LqExpr::Empty => Ok(()),
        LqExpr::Leaf(leaf) => validate_leaf_surface(leaf, policy),
        LqExpr::Not(inner) => validate_executable_text_surface(inner, policy),
        LqExpr::All(children) | LqExpr::Any(children) => {
            for child in children {
                validate_executable_text_surface(child, policy)?;
            }
            Ok(())
        }
    }
}

fn validate_leaf_surface(
    leaf: &LqLeaf,
    policy: ExecutableTextPlanePolicy,
) -> Result<(), CoreError> {
    let plane = policy.plane_name();
    match leaf {
        LqLeaf::Keyword(_) | LqLeaf::Phrase(_) | LqLeaf::RawString(_) => Ok(()),
        LqLeaf::Regex(_) => Err(CoreError::NotImplemented(format!(
            "{plane}: regex leaves are not executable on the current adapter set"
        ))),
        LqLeaf::StructuralBlock(_) => Err(CoreError::NotImplemented(format!(
            "{plane}: structural leaves are not executable on this route"
        ))),
        LqLeaf::Predicate { .. } => Err(CoreError::NotImplemented(format!(
            "{plane}: predicate leaves are not executable on this route"
        ))),
    }
}

/// One history element's position under the recency order (QI-BB-023).
///
/// Newest committer time first, then sha, then — for diffs — path. The
/// derived `Ord` is the wire contract's order, so a cursor is a rank and
/// "after the cursor" is `>`.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct HistoryRank {
    newest_first: std::cmp::Reverse<u64>,
    sha: CommitSha,
    file_path: Option<Box<str>>,
}

impl HistoryRank {
    fn for_commit(record: &quanta_index_contract::lex::CommitRecord) -> Self {
        Self {
            newest_first: std::cmp::Reverse(record.committer_time_ms),
            sha: record.sha,
            file_path: None,
        }
    }

    fn for_diff(
        commit: &quanta_index_contract::lex::CommitRecord,
        key: &crate::readiness::HistoryDiffKey,
    ) -> Self {
        Self {
            newest_first: std::cmp::Reverse(commit.committer_time_ms),
            sha: commit.sha,
            file_path: Some(key.file_path().into()),
        }
    }

    fn from_cursor(cursor: &HistoryCursor) -> Self {
        Self {
            newest_first: std::cmp::Reverse(cursor.committer_time_ms),
            sha: cursor.sha,
            file_path: cursor.file_path.as_deref().map(Into::into),
        }
    }

    fn into_cursor(self) -> HistoryCursor {
        HistoryCursor {
            committer_time_ms: self.newest_first.0,
            sha: self.sha,
            file_path: self.file_path.map(Into::into),
        }
    }
}

/// Keeps the `limit` smallest ranks (the newest elements) of everything
/// pushed, in `O(log limit)` per push, and counts what it saw.
struct HistoryPageSelector {
    limit: usize,
    kept: std::collections::BinaryHeap<HistoryRank>,
    matched: u64,
    examined: u64,
    after: Option<HistoryRank>,
}

impl HistoryPageSelector {
    fn new(limit: usize, cursor: Option<&HistoryCursor>) -> Self {
        Self {
            limit,
            kept: std::collections::BinaryHeap::new(),
            matched: 0,
            examined: 0,
            after: cursor.map(HistoryRank::from_cursor),
        }
    }

    fn examined_one(&mut self) {
        self.examined = self.examined.saturating_add(1);
    }

    /// Offer a matching element; elements at or before the cursor are
    /// already on an earlier page.
    fn offer(&mut self, rank: HistoryRank) {
        if self.after.as_ref().is_some_and(|after| rank <= *after) {
            return;
        }
        self.matched = self.matched.saturating_add(1);
        self.kept.push(rank);
        if self.kept.len() > self.limit {
            // The heap's max is the oldest kept element; it leaves.
            drop(self.kept.pop());
        }
    }

    /// The page in order, its window and its continuation.
    fn finish(
        self,
    ) -> Result<(Vec<HistoryRank>, QueryResultWindowV1, Option<HistoryCursor>), CoreError> {
        let ranks = self.kept.into_sorted_vec();
        let returned = u32::try_from(ranks.len()).map_err(|error| {
            CoreError::Storage(format!("history: page row count overflows u32: {error}"))
        })?;
        let has_more = self.matched > u64::from(returned);
        let window = QueryResultWindowV1::new(
            returned,
            quanta_index_contract::CandidateCountV1::Exact(self.matched),
            has_more,
        )
        .map_err(|error| CoreError::Storage(format!("history: result window: {error}")))?;
        let next_cursor = if has_more {
            ranks.last().cloned().map(HistoryRank::into_cursor)
        } else {
            None
        };
        Ok((ranks, window, next_cursor))
    }
}

/// One page of history results in recency order.
#[derive(Debug)]
struct HistoryPage {
    commits: Vec<CommitCandidate>,
    diffs: Vec<DiffCandidate>,
    window: QueryResultWindowV1,
    examined: u64,
    next_cursor: Option<HistoryCursor>,
}

/// Evaluate every record of the queried kind, keep the `top_k` newest
/// matches after `cursor`, and return them in recency order with an exact
/// match count (QI-BB-023).
///
/// The scan is complete on purpose: the authority is keyed by sha, so the
/// newest matches can be anywhere in it, and the count the window
/// reports is exact rather than a bound.
fn execute_history_query(
    query: &LqQuery,
    state: &HistoryAuthorityState,
    top_k: u32,
    cursor: Option<&HistoryCursor>,
) -> Result<HistoryPage, CoreError> {
    let kind = resolve_history_query_kind(query)?;
    let limit = top_k_limit(top_k);
    let mut selector = HistoryPageSelector::new(limit, cursor);
    match kind {
        HistoryQueryKind::Commit => {
            if cursor.is_some_and(|cursor| cursor.file_path.is_some()) {
                return Err(history_invalid_request(
                    "history: a commit page cannot continue from a diff cursor",
                ));
            }
            for record in state.commits().values() {
                selector.examined_one();
                if history_commit_matches(query, state, record)? {
                    selector.offer(HistoryRank::for_commit(record));
                }
            }
            let examined = selector.examined;
            let (ranks, window, next_cursor) = selector.finish()?;
            let commits = ranks
                .iter()
                .map(|rank| {
                    state
                        .commits()
                        .get(&rank.sha)
                        .map(commit_candidate_from_record)
                        .ok_or_else(|| {
                            CoreError::Storage(format!(
                                "history: selected commit {} vanished from the snapshot",
                                rank.sha
                            ))
                        })
                })
                .collect::<Result<Vec<_>, CoreError>>()?;
            Ok(HistoryPage {
                commits,
                diffs: Vec::new(),
                window,
                examined,
                next_cursor,
            })
        }
        HistoryQueryKind::Diff => {
            if cursor.is_some_and(|cursor| cursor.file_path.is_none()) {
                return Err(history_invalid_request(
                    "history: a diff page cannot continue from a commit cursor",
                ));
            }
            for (key, record) in state.diff_hunks() {
                selector.examined_one();
                let Some(commit) = state.commits().get(&key.commit_sha()) else {
                    continue;
                };
                if history_diff_matches(query, state, key, record, commit)? {
                    selector.offer(HistoryRank::for_diff(commit, key));
                }
            }
            let examined = selector.examined;
            let (ranks, window, next_cursor) = selector.finish()?;
            let diffs = ranks
                .iter()
                .map(|rank| {
                    let path = rank.file_path.as_deref().ok_or_else(|| {
                        CoreError::Storage("history: a diff rank carries no path".to_string())
                    })?;
                    let key = crate::readiness::HistoryDiffKey::new(rank.sha, path);
                    state
                        .diff_hunks()
                        .get(&key)
                        .map(|record| diff_candidate_from_record(&key, record))
                        .ok_or_else(|| {
                            CoreError::Storage(format!(
                                "history: selected diff {}:{path} vanished from the snapshot",
                                rank.sha
                            ))
                        })
                })
                .collect::<Result<Vec<_>, CoreError>>()?;
            Ok(HistoryPage {
                commits: Vec::new(),
                diffs,
                window,
                examined,
                next_cursor,
            })
        }
    }
}

fn execute_runtime_metadata_query(
    pin: &GenerationPin,
    query: &LqQuery,
    runtime_state: &RuntimeMetadataState,
    structural_state: &StructuralAuthorityState,
    top_k: u32,
) -> Result<Vec<quanta_index_contract::LexicalCandidate>, CoreError> {
    let limit = top_k_limit(top_k);
    let mut out = Vec::new();
    let seed_ids = runtime_seed_ids(query, runtime_state, structural_state)?;
    for chunk_id in seed_ids {
        let chunk = structural_state.chunks().get(&chunk_id).ok_or_else(|| {
            CoreError::InvalidContract(format!(
                "runtime metadata: seeded chunk `{}` is missing from lexical chunk authority",
                chunk_id.as_str()
            ))
        })?;
        if !runtime_chunk_matches(query, runtime_state, &chunk_id, chunk)? {
            continue;
        }
        out.push(lexical_candidate_from_chunk(pin, chunk));
        if out.len() >= limit {
            break;
        }
    }
    Ok(out)
}

fn history_query_type(query: &LqQuery) -> Option<LqType> {
    for filter in &query.filters {
        if let LqFilter::Type { kind } = filter {
            return Some(*kind);
        }
    }
    None
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HistoryQueryKind {
    Commit,
    Diff,
}

fn resolve_history_query_kind(query: &LqQuery) -> Result<HistoryQueryKind, CoreError> {
    let has_diff_only_filters = query.filters.iter().any(|filter| {
        matches!(
            filter,
            LqFilter::File { .. }
                | LqFilter::DiffAdded { .. }
                | LqFilter::DiffRemoved { .. }
                | LqFilter::DiffTouched { .. }
        )
    });
    match history_query_type(query) {
        Some(LqType::Commit) => {
            if has_diff_only_filters {
                return Err(history_invalid_request(
                    "history: `file:` and `diff.*` filters require `type:diff`",
                ));
            }
            Ok(HistoryQueryKind::Commit)
        }
        Some(LqType::Diff) => Ok(HistoryQueryKind::Diff),
        Some(LqType::File | LqType::Path | LqType::Symbol | LqType::Repo) => Err(
            history_invalid_request("history: only `type:commit` and `type:diff` are executable"),
        ),
        None => Err(history_invalid_request(
            "history: explicit `type:commit` or `type:diff` is required",
        )),
    }
}

fn history_commit_matches(
    query: &LqQuery,
    state: &HistoryAuthorityState,
    record: &quanta_index_contract::lex::CommitRecord,
) -> Result<bool, CoreError> {
    for filter in &query.filters {
        match filter {
            LqFilter::Type { kind } => {
                if !matches!(kind, LqType::Commit) {
                    return Ok(false);
                }
            }
            LqFilter::File { .. } => return Ok(false),
            LqFilter::Rev { spec } => {
                if !history_rev_matches(state, spec, &record.sha) {
                    return Ok(false);
                }
            }
            LqFilter::Author { pattern } => {
                if !matches_text(pattern, record.author.as_ref(), &query.options) {
                    return Ok(false);
                }
            }
            LqFilter::Committer { pattern } => {
                if !matches_text(pattern, record.committer.as_ref(), &query.options) {
                    return Ok(false);
                }
            }
            LqFilter::Message { pattern } => {
                if !matches_text(pattern, record.message.as_ref(), &query.options) {
                    return Ok(false);
                }
            }
            LqFilter::Content { leaf } => {
                if !leaf_matches_text("history", leaf, record.message.as_ref(), &query.options)? {
                    return Ok(false);
                }
            }
            LqFilter::Before { timeref } => {
                if !history_committer_time_before(record.committer_time_ms, timeref)? {
                    return Ok(false);
                }
            }
            LqFilter::After { timeref } => {
                if !history_committer_time_after(record.committer_time_ms, timeref)? {
                    return Ok(false);
                }
            }
            LqFilter::Since { timeref } => {
                if !history_committer_time_since(state, record.committer_time_ms, timeref)? {
                    return Ok(false);
                }
            }
            LqFilter::Until { timeref } => {
                if !history_committer_time_until(record.committer_time_ms, timeref)? {
                    return Ok(false);
                }
            }
            LqFilter::DiffAdded { .. }
            | LqFilter::DiffRemoved { .. }
            | LqFilter::DiffTouched { .. } => {
                return Ok(false);
            }
            LqFilter::Repo { .. }
            | LqFilter::Lang { .. }
            | LqFilter::Select { .. }
            | LqFilter::Dirty { .. }
            | LqFilter::Changed { .. }
            | LqFilter::Stale { .. }
            | LqFilter::Snapshot { .. }
            | LqFilter::MetaOwner { .. }
            | LqFilter::MetaService { .. }
            | LqFilter::MetaLayer { .. }
            | LqFilter::MetaSurface { .. }
            | LqFilter::Affected { .. }
            | LqFilter::InvalidatedBy { .. }
            | LqFilter::Fork { .. }
            | LqFilter::Archived { .. }
            | LqFilter::Visibility { .. }
            | LqFilter::Context { .. } => {}
        }
    }
    expr_matches(&query.expr, &mut |leaf| {
        leaf_matches_text("history", leaf, record.message.as_ref(), &query.options)
    })
}

fn history_diff_matches(
    query: &LqQuery,
    state: &HistoryAuthorityState,
    key: &crate::readiness::HistoryDiffKey,
    record: &quanta_index_contract::lex::DiffHunkRecord,
    commit: &quanta_index_contract::lex::CommitRecord,
) -> Result<bool, CoreError> {
    for filter in &query.filters {
        match filter {
            LqFilter::Type { kind } => {
                if !matches!(kind, LqType::Diff) {
                    return Ok(false);
                }
            }
            LqFilter::File { pattern, .. } => {
                if !matches_text(pattern, key.file_path(), &query.options) {
                    return Ok(false);
                }
            }
            LqFilter::Rev { spec } => {
                if !history_rev_matches(state, spec, &commit.sha) {
                    return Ok(false);
                }
            }
            LqFilter::Author { pattern } => {
                if !matches_text(pattern, commit.author.as_ref(), &query.options) {
                    return Ok(false);
                }
            }
            LqFilter::Committer { pattern } => {
                if !matches_text(pattern, commit.committer.as_ref(), &query.options) {
                    return Ok(false);
                }
            }
            LqFilter::Message { pattern } => {
                if !matches_text(pattern, commit.message.as_ref(), &query.options) {
                    return Ok(false);
                }
            }
            LqFilter::Content { leaf } => {
                if !leaf_matches_text(
                    "history",
                    leaf,
                    &history_diff_search_text(key, record),
                    &query.options,
                )? {
                    return Ok(false);
                }
            }
            LqFilter::Before { timeref } => {
                if !history_committer_time_before(commit.committer_time_ms, timeref)? {
                    return Ok(false);
                }
            }
            LqFilter::After { timeref } => {
                if !history_committer_time_after(commit.committer_time_ms, timeref)? {
                    return Ok(false);
                }
            }
            LqFilter::Since { timeref } => {
                if !history_committer_time_since(state, commit.committer_time_ms, timeref)? {
                    return Ok(false);
                }
            }
            LqFilter::Until { timeref } => {
                if !history_committer_time_until(commit.committer_time_ms, timeref)? {
                    return Ok(false);
                }
            }
            LqFilter::DiffAdded { pattern } => {
                if !matches_text(pattern, record.added_text.as_ref(), &query.options) {
                    return Ok(false);
                }
            }
            LqFilter::DiffRemoved { pattern } => {
                if !matches_text(pattern, record.removed_text.as_ref(), &query.options) {
                    return Ok(false);
                }
            }
            LqFilter::DiffTouched { pattern } => {
                if !matches_text(pattern, record.touched_text.as_ref(), &query.options) {
                    return Ok(false);
                }
            }
            LqFilter::Repo { .. }
            | LqFilter::Lang { .. }
            | LqFilter::Select { .. }
            | LqFilter::Dirty { .. }
            | LqFilter::Changed { .. }
            | LqFilter::Stale { .. }
            | LqFilter::Snapshot { .. }
            | LqFilter::MetaOwner { .. }
            | LqFilter::MetaService { .. }
            | LqFilter::MetaLayer { .. }
            | LqFilter::MetaSurface { .. }
            | LqFilter::Affected { .. }
            | LqFilter::InvalidatedBy { .. }
            | LqFilter::Fork { .. }
            | LqFilter::Archived { .. }
            | LqFilter::Visibility { .. }
            | LqFilter::Context { .. } => {}
        }
    }
    let diff_text = history_diff_search_text(key, record);
    expr_matches(&query.expr, &mut |leaf| {
        leaf_matches_text("history", leaf, &diff_text, &query.options)
    })
}

fn runtime_chunk_matches(
    query: &LqQuery,
    runtime_state: &RuntimeMetadataState,
    chunk_id: &quanta_index_contract::ChunkId,
    chunk: &ChunkRecord,
) -> Result<bool, CoreError> {
    for filter in &query.filters {
        match filter {
            LqFilter::Dirty { mode } => {
                let in_dirty = runtime_state.dirty_docs().contains_key(chunk_id);
                match mode {
                    LqYesNoOnly::No => {
                        if in_dirty {
                            return Ok(false);
                        }
                    }
                    // `Yes` (is-dirty) and `Only` (dirty-only) share the same
                    // membership requirement for this predicate: the chunk must
                    // be in the dirty set, else it is filtered out.
                    LqYesNoOnly::Yes | LqYesNoOnly::Only => {
                        if !in_dirty {
                            return Ok(false);
                        }
                    }
                }
            }
            LqFilter::Changed { scope } => {
                let since_ms = parse_runtime_changed_scope_ms(scope)?;
                let Some(record) = runtime_state.changed_docs().get(chunk_id) else {
                    return Ok(false);
                };
                if record.applied_at_ms() < since_ms {
                    return Ok(false);
                }
            }
            LqFilter::Stale { scope } => {
                let before_ms = parse_runtime_stale_scope_ms(scope)?;
                if !runtime_generation_is_stale(runtime_state, before_ms)? {
                    return Ok(false);
                }
            }
            LqFilter::Snapshot { name } => {
                if let Some(docs) = runtime_state.snapshots().get(name.as_str()) {
                    if !docs.contains(chunk_id) {
                        return Ok(false);
                    }
                } else {
                    return Err(runtime_snapshot_unknown(name));
                }
            }
            LqFilter::MetaOwner { id } => {
                if !runtime_doc_facet_matches(
                    runtime_state.doc_facets().get(chunk_id),
                    DocFacetState::owner,
                    id,
                ) {
                    return Ok(false);
                }
            }
            LqFilter::MetaService { id } => {
                if !runtime_doc_facet_matches(
                    runtime_state.doc_facets().get(chunk_id),
                    DocFacetState::service,
                    id,
                ) {
                    return Ok(false);
                }
            }
            LqFilter::MetaLayer { id } => {
                if !runtime_doc_facet_matches(
                    runtime_state.doc_facets().get(chunk_id),
                    DocFacetState::layer,
                    id,
                ) {
                    return Ok(false);
                }
            }
            LqFilter::MetaSurface { id } => {
                if !runtime_doc_facet_matches(
                    runtime_state.doc_facets().get(chunk_id),
                    DocFacetState::surface,
                    id,
                ) {
                    return Ok(false);
                }
            }
            LqFilter::Affected { scope } => {
                if !runtime_edge_matches(runtime_state.affected_docs(), scope, chunk_id) {
                    return Ok(false);
                }
            }
            LqFilter::InvalidatedBy { source } => {
                if !runtime_edge_matches(runtime_state.invalidated_by_docs(), source, chunk_id) {
                    return Ok(false);
                }
            }
            LqFilter::File { pattern, .. } => {
                if !matches_text(pattern, chunk.repo_relative_path.as_str(), &query.options) {
                    return Ok(false);
                }
            }
            LqFilter::Lang { id } => {
                if !matches_text(id, chunk.language.as_str(), &query.options) {
                    return Ok(false);
                }
            }
            LqFilter::Content { leaf } => {
                if !leaf_matches_text(
                    "runtime metadata",
                    leaf,
                    chunk.text.as_ref(),
                    &query.options,
                )? {
                    return Ok(false);
                }
            }
            LqFilter::Repo { .. }
            | LqFilter::Rev { .. }
            | LqFilter::Author { .. }
            | LqFilter::Committer { .. }
            | LqFilter::Message { .. }
            | LqFilter::Before { .. }
            | LqFilter::After { .. }
            | LqFilter::Since { .. }
            | LqFilter::Until { .. }
            | LqFilter::DiffAdded { .. }
            | LqFilter::DiffRemoved { .. }
            | LqFilter::DiffTouched { .. }
            | LqFilter::Type { .. }
            | LqFilter::Select { .. }
            | LqFilter::Fork { .. }
            | LqFilter::Archived { .. }
            | LqFilter::Visibility { .. }
            | LqFilter::Context { .. } => {}
        }
    }
    expr_matches(&query.expr, &mut |leaf| {
        leaf_matches_text(
            "runtime metadata",
            leaf,
            chunk.text.as_ref(),
            &query.options,
        )
    })
}

fn runtime_generation_is_stale(
    runtime_state: &RuntimeMetadataState,
    before_ms: u64,
) -> Result<bool, CoreError> {
    let Some(generation_materialized_at_ms) = runtime_state.generation_materialized_at_ms() else {
        return Err(runtime_catalog_head_missing(
            "generation_materialized_at_ms",
        ));
    };
    let Some(producer_head_applied_at_ms) = runtime_state.producer_head_applied_at_ms() else {
        return Err(runtime_catalog_head_missing("producer_head_applied_at_ms"));
    };
    Ok(producer_head_applied_at_ms > generation_materialized_at_ms
        && generation_materialized_at_ms < before_ms)
}

fn runtime_seed_ids(
    query: &LqQuery,
    runtime_state: &RuntimeMetadataState,
    structural_state: &StructuralAuthorityState,
) -> Result<BTreeSet<ChunkId>, CoreError> {
    let full_generation = structural_state
        .chunks()
        .keys()
        .cloned()
        .collect::<BTreeSet<_>>();
    let mut seed: Option<BTreeSet<ChunkId>> = None;
    for filter in &query.filters {
        let next = match filter {
            LqFilter::Dirty { mode } => match mode {
                // `Yes` (is-dirty) and `Only` (dirty-only) both seed from the
                // dirty-doc set; only `No` inverts against the full generation.
                LqYesNoOnly::Yes | LqYesNoOnly::Only => Some(
                    runtime_state
                        .dirty_docs()
                        .keys()
                        .cloned()
                        .collect::<BTreeSet<_>>(),
                ),
                LqYesNoOnly::No => Some(
                    full_generation
                        .iter()
                        .filter(|chunk_id| !runtime_state.dirty_docs().contains_key(*chunk_id))
                        .cloned()
                        .collect::<BTreeSet<_>>(),
                ),
            },
            LqFilter::Changed { .. } => Some(
                runtime_state
                    .changed_docs()
                    .keys()
                    .cloned()
                    .collect::<BTreeSet<_>>(),
            ),
            LqFilter::Stale { scope } => {
                let before_ms = parse_runtime_stale_scope_ms(scope)?;
                Some(if runtime_generation_is_stale(runtime_state, before_ms)? {
                    full_generation.clone()
                } else {
                    BTreeSet::new()
                })
            }
            LqFilter::Snapshot { name } => Some(
                runtime_state
                    .snapshots()
                    .get(name.as_str())
                    .cloned()
                    .ok_or_else(|| runtime_snapshot_unknown(name))?,
            ),
            LqFilter::MetaOwner { id } => Some(runtime_matching_facet_doc_ids(
                runtime_state,
                DocFacetState::owner,
                id,
            )),
            LqFilter::MetaService { id } => Some(runtime_matching_facet_doc_ids(
                runtime_state,
                DocFacetState::service,
                id,
            )),
            LqFilter::MetaLayer { id } => Some(runtime_matching_facet_doc_ids(
                runtime_state,
                DocFacetState::layer,
                id,
            )),
            LqFilter::MetaSurface { id } => Some(runtime_matching_facet_doc_ids(
                runtime_state,
                DocFacetState::surface,
                id,
            )),
            LqFilter::Affected { scope } => Some(
                runtime_state
                    .affected_docs()
                    .get(scope.as_str())
                    .cloned()
                    .unwrap_or_default(),
            ),
            LqFilter::InvalidatedBy { source } => Some(
                runtime_state
                    .invalidated_by_docs()
                    .get(source.as_str())
                    .cloned()
                    .unwrap_or_default(),
            ),
            LqFilter::File { .. }
            | LqFilter::Lang { .. }
            | LqFilter::Content { .. }
            | LqFilter::Repo { .. }
            | LqFilter::Rev { .. }
            | LqFilter::Author { .. }
            | LqFilter::Committer { .. }
            | LqFilter::Message { .. }
            | LqFilter::Before { .. }
            | LqFilter::After { .. }
            | LqFilter::Since { .. }
            | LqFilter::Until { .. }
            | LqFilter::DiffAdded { .. }
            | LqFilter::DiffRemoved { .. }
            | LqFilter::DiffTouched { .. }
            | LqFilter::Type { .. }
            | LqFilter::Select { .. }
            | LqFilter::Fork { .. }
            | LqFilter::Archived { .. }
            | LqFilter::Visibility { .. }
            | LqFilter::Context { .. } => None,
        };
        if let Some(next) = next {
            match &mut seed {
                Some(current) => current.retain(|chunk_id| next.contains(chunk_id)),
                None => seed = Some(next),
            }
        }
    }
    Ok(seed.unwrap_or(full_generation))
}

fn runtime_matching_facet_doc_ids(
    runtime_state: &RuntimeMetadataState,
    field: impl Fn(&DocFacetState) -> Option<&str>,
    expected: &str,
) -> BTreeSet<ChunkId> {
    runtime_state
        .doc_facets()
        .iter()
        .filter(|(_, facet)| field(facet) == Some(expected))
        .map(|(chunk_id, _)| chunk_id.clone())
        .collect()
}

fn runtime_doc_facet_matches(
    facet: Option<&DocFacetState>,
    field: impl Fn(&DocFacetState) -> Option<&str>,
    expected: &str,
) -> bool {
    facet.is_some_and(|facet| field(facet).is_some_and(|value| value == expected))
}

fn runtime_edge_matches(
    edges: &BTreeMap<Box<str>, BTreeSet<quanta_index_contract::ChunkId>>,
    expected: &str,
    chunk_id: &quanta_index_contract::ChunkId,
) -> bool {
    edges
        .get(expected)
        .is_some_and(|doc_ids| doc_ids.contains(chunk_id))
}

fn expr_matches<F>(expr: &LqExpr, leaf_matches: &mut F) -> Result<bool, CoreError>
where
    F: FnMut(&LqLeaf) -> Result<bool, CoreError>,
{
    match expr {
        LqExpr::Empty => Ok(true),
        LqExpr::Leaf(leaf) => leaf_matches(leaf),
        LqExpr::Not(inner) => Ok(!expr_matches(inner, leaf_matches)?),
        LqExpr::All(children) => {
            for child in children {
                if !expr_matches(child, leaf_matches)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        LqExpr::Any(children) => {
            for child in children {
                if expr_matches(child, leaf_matches)? {
                    return Ok(true);
                }
            }
            Ok(false)
        }
    }
}

fn leaf_matches_text(
    plane: &str,
    leaf: &LqLeaf,
    text: &str,
    options: &LqOptions,
) -> Result<bool, CoreError> {
    match leaf {
        LqLeaf::Keyword(value) | LqLeaf::Phrase(value) | LqLeaf::RawString(value) => {
            Ok(matches_text(value, text, options))
        }
        LqLeaf::Regex(_) => Err(CoreError::NotImplemented(format!(
            "{plane}: regex leaves are not executable on the current adapter set"
        ))),
        LqLeaf::StructuralBlock(_) => Err(CoreError::NotImplemented(format!(
            "{plane}: structural leaves are not executable on this route"
        ))),
        LqLeaf::Predicate { .. } => Err(CoreError::NotImplemented(format!(
            "{plane}: predicate leaves are not executable on this route"
        ))),
    }
}

fn matches_text(needle: &str, haystack: &str, options: &LqOptions) -> bool {
    if matches!(options.case, Some(LqCase::Insensitive)) {
        haystack
            .to_ascii_lowercase()
            .contains(&needle.to_ascii_lowercase())
    } else {
        haystack.contains(needle)
    }
}

fn history_rev_matches(state: &HistoryAuthorityState, spec: &str, sha: &CommitSha) -> bool {
    if sha.to_string() == spec {
        return true;
    }
    if state
        .refs()
        .get(spec)
        .is_some_and(|resolved| resolved == sha)
    {
        return true;
    }
    state
        .tags()
        .get(spec)
        .is_some_and(|resolved| resolved == sha)
}

fn commit_candidate_from_record(
    record: &quanta_index_contract::lex::CommitRecord,
) -> CommitCandidate {
    CommitCandidate {
        sha: record.sha,
        parent_ids: record.parents.clone(),
        committed_at_unix_s: unix_seconds_from_ms(record.committer_time_ms),
        author: record.author.to_string(),
        committer: record.committer.to_string(),
        message: record.message.to_string(),
        is_merge: record.is_merge,
        tags: record.tags.iter().map(ToString::to_string).collect(),
    }
}

fn diff_candidate_from_record(
    key: &crate::readiness::HistoryDiffKey,
    record: &quanta_index_contract::lex::DiffHunkRecord,
) -> DiffCandidate {
    DiffCandidate {
        repo_relative_path: key.file_path().to_string(),
        hunk_header: record.hunk_header.to_string(),
        side: record.side,
        line_start: record.byte_start,
        line_end: record.byte_end,
        snippet: history_diff_snippet(record),
    }
}

fn history_diff_search_text(
    key: &crate::readiness::HistoryDiffKey,
    record: &quanta_index_contract::lex::DiffHunkRecord,
) -> String {
    format!(
        "{}\n{}\n{}\n{}\n{}",
        key.file_path(),
        record.hunk_header,
        record.added_text,
        record.removed_text,
        record.touched_text
    )
}

fn history_diff_snippet(record: &quanta_index_contract::lex::DiffHunkRecord) -> String {
    if !record.touched_text.is_empty() {
        return record.touched_text.to_string();
    }
    if !record.added_text.is_empty() {
        return record.added_text.to_string();
    }
    if !record.removed_text.is_empty() {
        return record.removed_text.to_string();
    }
    record.hunk_header.to_string()
}

fn lexical_candidate_from_chunk(
    pin: &GenerationPin,
    chunk: &ChunkRecord,
) -> quanta_index_contract::LexicalCandidate {
    quanta_index_contract::LexicalCandidate {
        candidate_id: chunk.chunk_id.as_str().to_string(),
        repo_id: pin.repo_id.clone(),
        revision_id: pin.revision_id.clone(),
        manifest_generation: pin.manifest_generation,
        repo_relative_path: chunk.repo_relative_path.clone(),
        start_line: chunk.start_line,
        end_line: chunk.end_line,
        score: 1.0,
        snippet: chunk.derived_snippet().to_string(),
        // A projected chunk carries no single lexical hit anchor.
        snippet_hit_offset: None,
        highlights: Vec::new(),
    }
}

fn top_k_limit(top_k: u32) -> usize {
    usize::try_from(top_k).map_or(usize::MAX, core::convert::identity)
}

fn unix_seconds_from_ms(ms: u64) -> i64 {
    i64::try_from(ms.div_euclid(1_000)).map_or(i64::MAX, core::convert::identity)
}

fn history_invalid_timeref(message: impl Into<String>) -> CoreError {
    CoreError::Typed {
        code: ERR_HISTORY_INVALID_TIMEREF.to_string(),
        message: message.into(),
    }
}

fn history_committer_time_before(committer_time_ms: u64, timeref: &str) -> Result<bool, CoreError> {
    let boundary_ms = parse_history_timeref_ms(timeref)?;
    Ok(committer_time_ms < boundary_ms)
}

fn history_committer_time_after(committer_time_ms: u64, timeref: &str) -> Result<bool, CoreError> {
    let boundary_ms = parse_history_timeref_ms(timeref)?;
    Ok(committer_time_ms > boundary_ms)
}

fn history_committer_time_since(
    state: &HistoryAuthorityState,
    committer_time_ms: u64,
    timeref: &str,
) -> Result<bool, CoreError> {
    let boundary_ms = resolve_history_since_timeref_ms(state, timeref)?;
    Ok(committer_time_ms >= boundary_ms)
}

fn history_committer_time_until(committer_time_ms: u64, timeref: &str) -> Result<bool, CoreError> {
    let boundary_ms = parse_history_timeref_ms(timeref)?;
    Ok(committer_time_ms <= boundary_ms)
}

fn parse_history_timeref_ms(value: &str) -> Result<u64, CoreError> {
    if let Some(ms) = parse_rfc3339_timeref_ms(value) {
        return Ok(ms);
    }
    if let Some(ms) = parse_duration_timeref_ms(value) {
        return Ok(ms);
    }
    Err(history_invalid_timeref(format!(
        "history: timeref `{value}` is not a valid RFC3339 timestamp or duration"
    )))
}

fn validate_history_since_timeref(timeref: &str) -> Result<(), CoreError> {
    if let Some(spec) = timeref.strip_prefix("commit:") {
        if spec.is_empty() {
            return Err(history_invalid_timeref(
                "history: since.commit requires a non-empty commit/ref/tag spec",
            ));
        }
        return Ok(());
    }
    let timeref = timeref.strip_prefix("time:").unwrap_or(timeref);
    let _: u64 = parse_history_timeref_ms(timeref)?;
    Ok(())
}

fn resolve_history_since_timeref_ms(
    state: &HistoryAuthorityState,
    timeref: &str,
) -> Result<u64, CoreError> {
    if let Some(spec) = timeref.strip_prefix("commit:") {
        return resolve_history_commit_timeref_ms(state, spec);
    }
    let timeref = timeref.strip_prefix("time:").unwrap_or(timeref);
    parse_history_timeref_ms(timeref)
}

fn resolve_history_commit_timeref_ms(
    state: &HistoryAuthorityState,
    spec: &str,
) -> Result<u64, CoreError> {
    if let Ok(sha) = CommitSha::from_hex(spec)
        && let Some(record) = state.commits().get(&sha)
    {
        return Ok(record.committer_time_ms);
    }
    if let Some(sha) = state.refs().get(spec)
        && let Some(record) = state.commits().get(sha)
    {
        return Ok(record.committer_time_ms);
    }
    if let Some(sha) = state.tags().get(spec)
        && let Some(record) = state.commits().get(sha)
    {
        return Ok(record.committer_time_ms);
    }
    Err(history_invalid_timeref(format!(
        "history: since.commit `{spec}` does not resolve to a materialized commit"
    )))
}

fn parse_rfc3339_timeref_ms(value: &str) -> Option<u64> {
    if let Some(ms) = parse_rfc3339_datetime_ms(value) {
        return Some(ms);
    }
    parse_rfc3339_date_only_ms(value)
}

fn parse_rfc3339_date_only_ms(value: &str) -> Option<u64> {
    let (year, rest) = parse_year_prefix(value)?;
    let (month, day, rest) = parse_month_day(rest)?;
    if !rest.is_empty() {
        return None;
    }
    unix_ms_from_utc_parts(year, month, day, 0, 0, 0, 0)
}

fn parse_rfc3339_datetime_ms(value: &str) -> Option<u64> {
    let (year, rest) = parse_year_prefix(value)?;
    let (month, day, rest) = parse_month_day(rest)?;
    let rest = rest.strip_prefix('T')?;
    let (hour, minute, second, fraction_ms, rest) = parse_time_of_day(rest)?;
    if rest != "Z" {
        return None;
    }
    unix_ms_from_utc_parts(year, month, day, hour, minute, second, fraction_ms)
}

fn parse_year_prefix(value: &str) -> Option<(u32, &str)> {
    if value.len() < 5 || value.as_bytes().get(4) != Some(&b'-') {
        return None;
    }
    let Ok(year) = value.get(..4)?.parse::<u32>() else {
        return None;
    };
    Some((year, value.get(5..)?))
}

fn parse_month_day(rest: &str) -> Option<(u32, u32, &str)> {
    if rest.len() < 5 || rest.as_bytes().get(2) != Some(&b'-') {
        return None;
    }
    let Ok(month) = rest.get(..2)?.parse::<u32>() else {
        return None;
    };
    let Ok(day) = rest.get(3..5)?.parse::<u32>() else {
        return None;
    };
    Some((month, day, rest.get(5..)?))
}

fn parse_time_of_day(rest: &str) -> Option<(u32, u32, u32, u32, &str)> {
    if rest.len() < 8 || rest.as_bytes().get(2) != Some(&b':') {
        return None;
    }
    let Ok(hour) = rest.get(..2)?.parse::<u32>() else {
        return None;
    };
    if rest.as_bytes().get(5) != Some(&b':') {
        return None;
    }
    let Ok(minute) = rest.get(3..5)?.parse::<u32>() else {
        return None;
    };
    let mut second_end = 6usize;
    while rest
        .as_bytes()
        .get(second_end)
        .is_some_and(u8::is_ascii_digit)
    {
        second_end = second_end.checked_add(1)?;
    }
    let Ok(second) = rest.get(6..second_end)?.parse::<u32>() else {
        return None;
    };
    let mut fraction_ms = 0u32;
    let mut tail = rest.get(second_end..)?;
    if let Some(after_dot) = tail.strip_prefix('.') {
        tail = after_dot;
        let mut digits = 0u32;
        let mut places = 0u32;
        for ch in tail.chars() {
            let Some(digit) = ch.to_digit(10) else {
                break;
            };
            digits = digits.saturating_mul(10).saturating_add(digit);
            places = places.checked_add(1)?;
            tail = tail.get(ch.len_utf8()..)?;
        }
        if places == 0 {
            return None;
        }
        while places < 3 {
            digits = digits.saturating_mul(10);
            places = places.checked_add(1)?;
        }
        fraction_ms = digits;
    }
    Some((hour, minute, second, fraction_ms, tail))
}

fn unix_ms_from_utc_parts(
    year: u32,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    second: u32,
    fraction_ms: u32,
) -> Option<u64> {
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 59
    {
        return None;
    }
    let days = days_from_civil(year, month, day)?;
    let seconds = u64::from(days)
        .saturating_mul(86_400)
        .saturating_add(u64::from(hour).saturating_mul(3_600))
        .saturating_add(u64::from(minute).saturating_mul(60))
        .saturating_add(u64::from(second));
    seconds
        .checked_mul(1_000)?
        .checked_add(u64::from(fraction_ms))
}

fn days_from_civil(year: u32, month: u32, day: u32) -> Option<u32> {
    let mut y = i64::from(year);
    let m = i64::from(month);
    y = y.checked_sub(i64::from(m <= 2))?;
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let month_shift = if m > 2 { -3 } else { 9 };
    let doy = m
        .checked_add(month_shift)?
        .checked_mul(153)?
        .checked_add(2)?
        .div_euclid(5)
        .checked_add(i64::from(day))?
        .checked_sub(1)?;
    let doe = yoe
        .checked_mul(365)?
        .checked_add(yoe.div_euclid(4))?
        .checked_sub(yoe.div_euclid(100))?
        .checked_add(doy)?;
    let days = era
        .checked_mul(146_097)?
        .checked_add(doe)?
        .checked_sub(719_468)?;
    let Ok(value) = u32::try_from(days) else {
        return None;
    };
    Some(value)
}

fn parse_duration_timeref_ms(value: &str) -> Option<u64> {
    let split_at = value.as_bytes().iter().position(|b| !b.is_ascii_digit())?;
    let (digits, unit) = value.split_at(split_at);
    if digits.is_empty() {
        return None;
    }
    let Ok(amount) = digits.parse::<u64>() else {
        return None;
    };
    let unit_ms: u64 = match unit {
        "s" => 1_000,
        "m" => 60_000,
        "h" => 3_600_000,
        "d" => 86_400_000,
        "w" => 604_800_000,
        "mo" => 2_592_000_000,
        "y" => 31_536_000_000,
        _ => return None,
    };
    let duration_ms = amount.checked_mul(unit_ms)?;
    let Ok(elapsed) = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) else {
        return None;
    };
    let Ok(now_ms) = u64::try_from(elapsed.as_millis()) else {
        return None;
    };
    now_ms.checked_sub(duration_ms)
}

fn resolve_generation_selector_pin(
    activation_catalog: &ActivationCatalog,
    selector: &GenerationSelector,
    track: SearchPlaneTrackKind,
    plane: &str,
) -> Result<GenerationPin, CoreError> {
    match selector {
        GenerationSelector::Active {
            repo_id,
            revision_id,
        } => activation_catalog
            .resolve(repo_id, revision_id, track)
            .map_err(|err| match err {
                CoreError::NotReady(msg) => {
                    CoreError::NotReady(format!("{plane}: active generation unresolved: {msg}"))
                }
                err @ (CoreError::InvalidContract(_)
                | CoreError::Typed { .. }
                | CoreError::NotImplemented(_)
                | CoreError::NotFound(_)
                | CoreError::Storage(_)) => err,
            }),
        GenerationSelector::Pinned(pin) => Ok(pin.clone()),
    }
}

fn stabilize_ranked_candidates(results: &mut [LexicalCandidate]) {
    results.sort_by(|left, right| {
        right
            .score
            .total_cmp(&left.score)
            .then_with(|| {
                left.repo_relative_path
                    .as_str()
                    .cmp(right.repo_relative_path.as_str())
            })
            .then(left.start_line.cmp(&right.start_line))
            .then(left.end_line.cmp(&right.end_line))
            .then_with(|| left.candidate_id.as_str().cmp(right.candidate_id.as_str()))
    });
}

fn stabilize_semantic_seed_hits_v1(results: &mut [SemanticSearchHitV1]) {
    results.sort_by(|left, right| {
        right
            .candidate
            .score
            .total_cmp(&left.candidate.score)
            .then_with(|| {
                left.candidate
                    .repo_relative_path
                    .as_str()
                    .cmp(right.candidate.repo_relative_path.as_str())
            })
            .then(left.candidate.start_line.cmp(&right.candidate.start_line))
            .then(left.candidate.end_line.cmp(&right.candidate.end_line))
            .then_with(|| left.owner_id.as_str().cmp(right.owner_id.as_str()))
            .then_with(|| left.record_id.as_str().cmp(right.record_id.as_str()))
            .then_with(|| {
                left.candidate
                    .candidate_id
                    .as_str()
                    .cmp(right.candidate.candidate_id.as_str())
            })
    });
}

fn resolve_optional_selection(
    activation_catalog: &ActivationCatalog,
    generation: Option<GenerationPin>,
    generation_selector: Option<&GenerationSelector>,
    track: SearchPlaneTrackKind,
    plane: &str,
) -> Result<Option<GenerationPin>, CoreError> {
    let selector_pin = match generation_selector {
        Some(selector) => Some(resolve_generation_selector_pin(
            activation_catalog,
            selector,
            track,
            plane,
        )?),
        None => None,
    };
    match (generation, selector_pin) {
        (Some(pin), Some(selected)) if pin != selected => Err(CoreError::InvalidContract(format!(
            "{plane}: explicit generation pin does not match generation selector resolution"
        ))),
        (Some(pin), _) | (None, Some(pin)) => Ok(Some(pin)),
        (None, None) => Ok(None),
    }
}

fn resolve_semantic_selector_selection(
    activation_catalog: &ActivationCatalog,
    selector: &GenerationSelector,
    plane: &str,
) -> Result<SemanticSelection, CoreError> {
    match selector {
        GenerationSelector::Active {
            repo_id,
            revision_id,
        } => resolve_active_semantic_selection(activation_catalog, repo_id, revision_id, plane),
        GenerationSelector::Pinned(pin) => Ok(SemanticSelection {
            pin: pin.clone(),
            expected_manifest_digest: None,
        }),
    }
}

fn resolve_active_semantic_selection(
    activation_catalog: &ActivationCatalog,
    repo_id: &RepoId,
    revision_id: &RevisionId,
    plane: &str,
) -> Result<SemanticSelection, CoreError> {
    let record = activation_catalog
        .resolve_record(repo_id, revision_id, SearchPlaneTrackKind::Semantic)
        .map_err(|err| match err {
            CoreError::NotReady(msg) => {
                CoreError::NotReady(format!("{plane}: active generation unresolved: {msg}"))
            }
            err @ (CoreError::InvalidContract(_)
            | CoreError::Typed { .. }
            | CoreError::NotImplemented(_)
            | CoreError::NotFound(_)
            | CoreError::Storage(_)) => err,
        })?;
    Ok(selection_from_active_semantic_record(record))
}

fn selection_from_active_semantic_record(record: ActiveGenerationRecord) -> SemanticSelection {
    let pin = GenerationPin::new(
        record.repo_id.clone(),
        record.revision_id.clone(),
        record.manifest_generation,
    );
    SemanticSelection {
        pin,
        expected_manifest_digest: Some(record.manifest_digest),
    }
}

struct PreparedLexicalTextQuery {
    pin: GenerationPin,
    query: LqQuery,
    force_empty: bool,
}

/// The one executable lexical plan for a text request.
struct PlannedLexicalTextQuery {
    pin: GenerationPin,
    query: LqQuery,
    constraints: QueryConstraintSetV1,
    force_empty: bool,
}

/// Relative tolerance under which a candidate's carried score is the score
/// this plan emits for it.
const EXPLAIN_SCORE_TOLERANCE: f32 = 1e-5;

/// The explanation of a presence-only explain: what the lookup found and
/// nothing about scores, since no query was named.
fn build_presence_explanation(
    candidate_id: &str,
    presence: CandidatePresenceV1,
) -> SearchExplanation {
    let indexed = presence == CandidatePresenceV1::Indexed;
    SearchExplanation {
        planner_trace: vec![
            PlannerTraceEntry {
                stage: PlannerStage::Plan,
                detail: "explain.mode=presence_lookup".to_string(),
            },
            PlannerTraceEntry {
                stage: PlannerStage::Merge,
                detail: format!("explain.candidate_indexed={indexed}"),
            },
        ],
        engines_touched: vec![EngineTouched::Lexical],
        early_stop_reason: None,
        contributions: Vec::new(),
        ranker_weights_hash: [0u8; 32],
        strategy: "presence_lookup".to_string(),
        summary: if indexed {
            format!("candidate {candidate_id} is present in the lexical index (exact lookup)")
        } else {
            format!(
                "candidate {candidate_id} is NOT present in the lexical index (exact lookup: stale, removed, or never indexed)"
            )
        },
    }
}

/// The ranker inputs a lexical plan scores with, pinned as one digest: the
/// engine the plan runs on and the boost it applies. Two explanations with
/// equal hashes were scored under the same weights.
fn lexical_ranker_weights_hash_v1(options: &LqOptions) -> [u8; 32] {
    use sha2::Digest as _;
    let engine = if matches!(options.index_mode, Some(LqYesNoOnly::No)) {
        LexicalScoreEngineV1::UnindexedScan
    } else {
        LexicalScoreEngineV1::Bm25
    };
    let mut hasher = sha2::Sha256::new();
    hasher.update(b"quanta-index lexical ranker weights v1\n");
    hasher.update(b"engine=");
    hasher.update(engine.as_str().as_bytes());
    hasher.update(b"\nboost_millis=");
    match options.boost_millis {
        Some(millis) => hasher.update(millis.to_string().as_bytes()),
        None => hasher.update(b"none"),
    }
    hasher.update(b"\n");
    hasher.finalize().into()
}

/// The explanation of a scored explain: one contribution row per signal,
/// summing to the emitted score, and whether the candidate's carried score
/// is that score.
fn build_lexical_score_explanation(
    candidate_id: &str,
    carried_score: f32,
    options: &LqOptions,
    explained: &LexicalCandidateExplanationV1,
) -> Result<SearchExplanation, CoreError> {
    let mut planner_trace = vec![PlannerTraceEntry {
        stage: PlannerStage::Plan,
        detail: "explain.mode=lexical_score_trace".to_string(),
    }];
    let (indexed, matched) = match explained {
        LexicalCandidateExplanationV1::NotIndexed => (false, false),
        LexicalCandidateExplanationV1::NotMatched { .. } => (true, false),
        LexicalCandidateExplanationV1::Matched(_) => (true, true),
    };
    planner_trace.push(PlannerTraceEntry {
        stage: PlannerStage::Merge,
        detail: format!("explain.candidate_indexed={indexed}"),
    });
    planner_trace.push(PlannerTraceEntry {
        stage: PlannerStage::Merge,
        detail: format!("explain.candidate_matched={matched}"),
    });
    let (contributions, summary) = match explained {
        LexicalCandidateExplanationV1::NotIndexed => (
            Vec::new(),
            format!("candidate {candidate_id} is NOT present in the lexical index (exact lookup)"),
        ),
        LexicalCandidateExplanationV1::NotMatched { reason } => (
            Vec::new(),
            format!(
                "candidate {candidate_id} is present in the lexical index but the query does not match it: {reason}"
            ),
        ),
        LexicalCandidateExplanationV1::Matched(trace) => {
            if !trace.emitted_score.is_finite() {
                return Err(CoreError::Storage(format!(
                    "explain: the lexical engine emitted a non-finite score for {candidate_id}"
                )));
            }
            let tolerance = EXPLAIN_SCORE_TOLERANCE * carried_score.abs().max(1.0);
            let reconciled = (trace.emitted_score - carried_score).abs() <= tolerance;
            planner_trace.push(PlannerTraceEntry {
                stage: PlannerStage::Merge,
                detail: format!("explain.score_reconciled={reconciled}"),
            });
            let rows = vec![ExplanationRow {
                signal_name: format!("lexical.{}", trace.engine.as_str()).into_boxed_str(),
                signal_value: trace.engine_score,
                weight: trace.boost_factor,
                contribution: trace.emitted_score,
            }];
            let summary = if reconciled {
                format!(
                    "candidate {candidate_id} is present and scores {:.6} under the query ({} {:.6} x boost {:.3}); the candidate's carried score is this score",
                    trace.emitted_score,
                    trace.engine.as_str(),
                    trace.engine_score,
                    trace.boost_factor
                )
            } else {
                format!(
                    "candidate {candidate_id} is present and scores {:.6} under the query ({} {:.6} x boost {:.3}); the candidate's carried score {carried_score:.6} is not this plan's score (fused or scored under another plan)",
                    trace.emitted_score,
                    trace.engine.as_str(),
                    trace.engine_score,
                    trace.boost_factor
                )
            };
            (rows, summary)
        }
    };
    Ok(SearchExplanation {
        planner_trace,
        engines_touched: vec![EngineTouched::Lexical],
        early_stop_reason: None,
        contributions,
        ranker_weights_hash: lexical_ranker_weights_hash_v1(options),
        strategy: "lexical_score_trace".to_string(),
        summary,
    })
}

struct RevAtTimeSelection<'a> {
    timeref: &'a str,
    explicit_anchor: Option<&'a str>,
}

fn resolve_lexical_request_pin(
    activation_catalog: &ActivationCatalog,
    request: &TextQueryRequest,
    track: SearchPlaneTrackKind,
    plane: &str,
) -> Result<GenerationPin, CoreError> {
    resolve_optional_selection(
        activation_catalog,
        request.generation.clone(),
        request.generation_selector.as_ref(),
        track,
        plane,
    )?
    .ok_or_else(|| CoreError::InvalidContract(format!("{plane}: generation pin required")))
}

fn prepare_lexical_text_query_for_execution(
    activation_catalog: &ActivationCatalog,
    ledger: &RwLock<Ledger>,
    base_pin: &GenerationPin,
    query: LqQuery,
) -> Result<PreparedLexicalTextQuery, CoreError> {
    let Some(selection) = rev_at_time_selection(&query)? else {
        return Ok(PreparedLexicalTextQuery {
            pin: base_pin.clone(),
            query,
            force_empty: false,
        });
    };

    let boundary_ms = parse_search_timeref_ms(selection.timeref).ok_or_else(|| {
        history_invalid_timeref(format!(
            "history: timeref `{}` is not a valid RFC3339 timestamp, named date, human phrase, or duration",
            selection.timeref
        ))
    })?;

    let guard = ledger
        .read()
        .map_err(|err| CoreError::Storage(format!("search-plane ledger poisoned: {err}")))?;
    let Some(history_state) = guard.history_state(
        &base_pin.repo_id,
        &base_pin.revision_id,
        base_pin.manifest_generation,
    ) else {
        let lexical_materialized = guard.track_materialized(
            &base_pin.repo_id,
            &base_pin.revision_id,
            SearchPlaneTrackKind::Lexical,
        );
        return Err(history_absent_error(base_pin, lexical_materialized));
    };
    ensure_rev_at_time_history_ready(history_state, selection.explicit_anchor, base_pin)?;
    let anchor_sha =
        resolve_rev_at_time_anchor_sha(history_state, selection.explicit_anchor, base_pin)?;
    let stripped_query = strip_rev_filters(query);
    let Some(selected_commit_sha) =
        select_reachable_commit_at_or_before(history_state, anchor_sha, boundary_ms)?
    else {
        return Ok(PreparedLexicalTextQuery {
            pin: base_pin.clone(),
            query: stripped_query,
            force_empty: true,
        });
    };
    drop(guard);
    let rebound_revision = RevisionId::new(selected_commit_sha.to_hex());
    let rebound_pin = activation_catalog.resolve(
        &base_pin.repo_id,
        &rebound_revision,
        SearchPlaneTrackKind::Lexical,
    )?;
    Ok(PreparedLexicalTextQuery {
        pin: rebound_pin,
        query: stripped_query,
        force_empty: false,
    })
}

fn rev_at_time_selection(query: &LqQuery) -> Result<Option<RevAtTimeSelection<'_>>, CoreError> {
    let mut timeref: Option<&str> = None;
    let mut explicit_anchor: Option<&str> = None;
    for filter in &query.filters {
        let LqFilter::Rev { spec } = filter else {
            continue;
        };
        if let Some(payload) = parse_rev_at_time_spec(spec) {
            if timeref.replace(payload).is_some() {
                return Err(CoreError::InvalidContract(
                    "lexical: rev:at.time(...) accepts exactly one timeref selector".to_string(),
                ));
            }
            continue;
        }
        if explicit_anchor.replace(spec.as_str()).is_some() {
            return Err(CoreError::InvalidContract(
                "lexical: rev:at.time(...) accepts at most one explicit rev anchor".to_string(),
            ));
        }
    }
    Ok(timeref.map(|timeref| RevAtTimeSelection {
        timeref,
        explicit_anchor,
    }))
}

fn strip_rev_filters(mut query: LqQuery) -> LqQuery {
    query
        .filters
        .retain(|filter| !matches!(filter, LqFilter::Rev { .. }));
    query
}

fn ensure_rev_at_time_history_ready(
    state: &HistoryAuthorityState,
    explicit_anchor: Option<&str>,
    base_pin: &GenerationPin,
) -> Result<(), CoreError> {
    if !state.commits_materialized() {
        return Err(history_shard_unavailable(
            "history: commit shard is unavailable for rev:at.time(...) selection",
        ));
    }
    let requires_lookup_shards = explicit_anchor
        .is_some_and(|spec| CommitSha::from_hex(spec).is_err())
        || CommitSha::from_hex(base_pin.revision_id.as_str()).is_err();
    if requires_lookup_shards && !state.refs_materialized() {
        return Err(history_shard_unavailable(
            "history: ref shard is unavailable for rev:at.time(...) selection",
        ));
    }
    if requires_lookup_shards && !state.tags_materialized() {
        return Err(history_shard_unavailable(
            "history: tag shard is unavailable for rev:at.time(...) selection",
        ));
    }
    Ok(())
}

fn resolve_rev_at_time_anchor_sha(
    state: &HistoryAuthorityState,
    explicit_anchor: Option<&str>,
    base_pin: &GenerationPin,
) -> Result<CommitSha, CoreError> {
    if let Some(spec) = explicit_anchor {
        return resolve_history_anchor_sha(state, spec).ok_or_else(|| {
            CoreError::InvalidContract(format!(
                "lexical: rev:at.time(...) anchor `{spec}` does not resolve to a materialized commit"
            ))
        });
    }
    if let Some(anchor_sha) = resolve_history_anchor_sha(state, base_pin.revision_id.as_str()) {
        return Ok(anchor_sha);
    }
    if let Some(anchor_sha) = state.refs().get("HEAD") {
        return Ok(*anchor_sha);
    }
    Err(CoreError::InvalidContract(
        "lexical: rev:at.time(...) requires the selected revision to be a materialized commit/ref/tag or a materialized HEAD ref".to_string(),
    ))
}

fn resolve_history_anchor_sha(state: &HistoryAuthorityState, spec: &str) -> Option<CommitSha> {
    if let Ok(sha) = CommitSha::from_hex(spec)
        && state.commits().contains_key(&sha)
    {
        return Some(sha);
    }
    state
        .refs()
        .get(spec)
        .copied()
        .or_else(|| state.tags().get(spec).copied())
}

fn select_reachable_commit_at_or_before(
    state: &HistoryAuthorityState,
    anchor_sha: CommitSha,
    boundary_ms: u64,
) -> Result<Option<CommitSha>, CoreError> {
    let mut frontier = vec![anchor_sha];
    let mut visited = BTreeSet::new();
    let mut best: Option<(u64, CommitSha)> = None;
    while let Some(current_sha) = frontier.pop() {
        if !visited.insert(current_sha) {
            continue;
        }
        let Some(record) = state.commits().get(&current_sha) else {
            return Err(history_shard_unavailable(
                "history: rev:at.time(...) anchor traversal encountered an unmapped commit shard entry",
            ));
        };
        if record.committer_time_ms <= boundary_ms {
            match best {
                Some((best_time, best_sha))
                    if best_time > record.committer_time_ms
                        || (best_time == record.committer_time_ms && best_sha >= current_sha) => {}
                _ => best = Some((record.committer_time_ms, current_sha)),
            }
        }
        frontier.extend(record.parents.iter().copied());
    }
    Ok(best.map(|(_, sha)| sha))
}

#[cfg(test)]
fn build_probe_query(probe_text: &str) -> LqQuery {
    use quanta_index_contract::LqSpan;
    LqQuery {
        lq_version: LQ_VERSION_TAG,
        expr: LqExpr::Leaf(LqLeaf::Phrase(probe_text.to_string())),
        filters: Vec::new(),
        directives: Vec::new(),
        options: LqOptions::defaults(),
        source_span: LqSpan::eof(u32::try_from(probe_text.len()).map_or(u32::MAX, |n| n)),
    }
}

fn metric_count_value(count: usize) -> f64 {
    u32::try_from(count).map_or_else(|_| f64::from(u32::MAX), f64::from)
}

/// Construct a [`GenerationPin`] from primitives. Public helper used in tests.
#[must_use]
pub fn make_pin(
    repo_id: RepoId,
    revision_id: RevisionId,
    manifest_generation: ManifestGeneration,
) -> GenerationPin {
    GenerationPin::new(repo_id, revision_id, manifest_generation)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::sync::{Arc, Mutex, RwLock};

    use super::{
        BoundedQueryObsStore, ERR_HISTORY_GENERATION_NOT_READY, ERR_HISTORY_INVALID_TIMEREF,
        ERR_HISTORY_PRODUCER_UNAVAILABLE, ERR_HISTORY_SHARD_UNAVAILABLE, ERR_INVALID,
        ERR_NOT_IMPLEMENTED, ERR_NOT_READY, ERR_RUNTIME_DIRTY_ONLY_UNSUPPORTED,
        FailClosedStructuralProducer, LexicalSearchPageV1, MAX_OBS_SAMPLES, QueryObsSink,
        SearchPlaneDispatcher, build_hybrid_seed_candidates, build_probe_query,
        classify_error_metric_name, finalize_probe_window_v1, fused_window_v1,
        lexical_fetch_limit_v1, lexical_page_window_v1, make_pin, prepare_language_query_v1,
        probe_top_k_v1, runtime_generation_is_stale, runtime_seed_ids, validate_history_query,
        validate_runtime_metadata_query,
    };
    use crate::{
        ActivationCatalog, HashingQueryTextEmbedder, Ledger, PreparedSearchCorpusGenerationV1,
        QueryTextEmbedderPort, SEARCH_OWNED_SEMANTIC_DIMENSION, SearchCorpusGenerationV1,
        SnapshotRegistries, SnapshotRegistryPolicy,
    };
    use quanta_index_contract::{
        ClusterMembershipBatchReadRequestV1, ClusterMembershipBatchReadResponseV1,
        HybridSeedQueryRequest, INTERNAL_FETCH_CEILING, PUBLIC_TOP_K_MAX, QueryConstraintSetV1,
        SymbolQueryRequest, TOP_K_OUT_OF_RANGE_CODE,
    };
    use quanta_index_core::{
        DenseIndexV1, DenseLaneAttestationV1, DenseLaneContractV1, REQUEST_CANCELLED_CODE,
        REQUEST_DEADLINE_EXCEEDED_CODE, RequestBudgetV1, SemanticSearchHitV1,
    };

    #[test]
    fn typed_and_dsl_language_constraints_intersect_before_every_retrieval_lane_v1() {
        use quanta_index_contract::lex::LanguageCode;
        use quanta_index_contract::{LqFilter, QueryConstraintSetV1};

        let typed = QueryConstraintSetV1::from_languages([
            LanguageCode::new("rust").expect("valid language"),
            LanguageCode::new("python").expect("valid language"),
        ])
        .with_exact_repo_relative_path(
            quanta_index_contract::ExactRepoRelativePathV1::new("src/lib.rs")
                .expect("valid exact path"),
        );
        let mut query = build_probe_query("needle");
        query.filters.push(LqFilter::Lang {
            id: "Rust".to_string(),
        });
        let prepared = prepare_language_query_v1(query, &typed).expect("valid constraints");
        assert!(!prepared.force_empty);
        assert!(prepared.query.filters.is_empty());
        assert_eq!(
            prepared
                .constraints
                .language_any_of
                .iter()
                .map(LanguageCode::as_str)
                .collect::<Vec<_>>(),
            vec!["rust"]
        );
        assert_eq!(
            prepared
                .constraints
                .repo_relative_path_exact
                .as_ref()
                .map(quanta_index_contract::ExactRepoRelativePathV1::as_str),
            Some("src/lib.rs"),
            "DSL language composition must preserve the independent path axis"
        );

        let typed_rust = QueryConstraintSetV1::from_languages([
            LanguageCode::new("rust").expect("valid language")
        ]);
        let mut disjoint = build_probe_query("needle");
        disjoint.filters.push(LqFilter::Lang {
            id: "python".to_string(),
        });
        let prepared = prepare_language_query_v1(disjoint, &typed_rust).expect("valid constraints");
        assert!(
            prepared.force_empty,
            "disjoint constraints must not widen to all languages"
        );
        assert!(prepared.constraints.is_unconstrained());
    }

    #[test]
    fn query_window_uses_one_continuation_row_and_never_requires_full_count_v1() {
        use quanta_index_contract::{CandidateCountV1, QueryResultWindowV1};

        let mut exact = vec![1_u8, 2];
        assert_eq!(
            finalize_probe_window_v1(&mut exact, 3).expect("valid exact window"),
            QueryResultWindowV1::exact(2)
        );
        let mut continued = vec![1_u8, 2, 3, 4];
        let window = finalize_probe_window_v1(&mut continued, 3).expect("valid lower bound");
        assert_eq!(continued, vec![1, 2, 3]);
        assert_eq!(window.returned(), 3);
        assert_eq!(window.candidate_count(), CandidateCountV1::AtLeast(4));
        assert!(window.has_more());
        let fused = fused_window_v1(100, 100, 100, true).expect("capped lane is a lower bound");
        assert_eq!(fused.candidate_count(), CandidateCountV1::AtLeast(101));
        assert!(fused.has_more());
        assert!(fused_window_v1(100, 99, 99, true).is_err());
        assert_eq!(
            probe_top_k_v1(9_999).expect("one-row probe within ceiling"),
            10_000
        );
        // The public maximum is accepted and probes one row past it; the
        // internal fetch ceiling is the contract's, not the caller's.
        assert_eq!(
            probe_top_k_v1(PUBLIC_TOP_K_MAX).expect("public maximum is accepted"),
            INTERNAL_FETCH_CEILING
        );
        for refused in [0, PUBLIC_TOP_K_MAX + 1, u32::MAX] {
            match probe_top_k_v1(refused) {
                Err(CoreError::Typed { code, .. }) => assert_eq!(code, TOP_K_OUT_OF_RANGE_CODE),
                other => {
                    panic!("top_k={refused} must be refused with the shared code, got {other:?}")
                }
            }
        }
    }

    #[test]
    fn count_options_take_an_exact_window_from_the_adapter_and_never_widen_the_page_v1() {
        use quanta_index_contract::{CandidateCountV1, LqCountBound};

        let mut query = build_probe_query("needle");
        query.options.count = Some(LqCountBound::All);
        assert_eq!(
            lexical_fetch_limit_v1(&query, 1).expect("count:all fetch limit"),
            1,
            "an exact total makes the continuation probe unnecessary"
        );
        query.options.count = None;
        assert_eq!(
            lexical_fetch_limit_v1(&query, 1).expect("plain fetch limit"),
            2,
            "without a count the page carries one probe row"
        );

        // The adapter proved three matches but the page is one row.
        let mut page = LexicalSearchPageV1 {
            candidates: vec![candidate("alpha", 1.0)],
            exact_total: Some(3),
        };
        let window = lexical_page_window_v1(&mut page, 1, 1).expect("exact window");
        assert_eq!(window.returned(), 1);
        assert_eq!(window.candidate_count(), CandidateCountV1::Exact(3));
        assert!(window.has_more());

        // A projection fetched with a probe row still cuts to the page and
        // keeps the exact total.
        let mut projected = LexicalSearchPageV1 {
            candidates: vec![candidate("alpha", 1.0), candidate("beta", 0.5)],
            exact_total: Some(5),
        };
        let window = lexical_page_window_v1(&mut projected, 1, 2).expect("projected window");
        assert_eq!(projected.candidates.len(), 1);
        assert_eq!(window.candidate_count(), CandidateCountV1::Exact(5));
        assert!(window.has_more());

        // An adapter that returns more rows than it was asked for is a contract defect.
        let mut oversized = LexicalSearchPageV1 {
            candidates: vec![candidate("alpha", 1.0), candidate("beta", 0.5)],
            exact_total: Some(2),
        };
        assert!(lexical_page_window_v1(&mut oversized, 1, 1).is_err());

        // An exact total below the returned rows is a contract defect.
        let mut contradictory = LexicalSearchPageV1 {
            candidates: vec![candidate("alpha", 1.0), candidate("beta", 0.5)],
            exact_total: Some(1),
        };
        assert!(lexical_page_window_v1(&mut contradictory, 5, 6).is_err());

        // Without an exact total the probe row is consumed into `has_more`.
        let mut probed = LexicalSearchPageV1 {
            candidates: vec![candidate("alpha", 1.0), candidate("beta", 0.5)],
            exact_total: None,
        };
        let window = lexical_page_window_v1(&mut probed, 1, 2).expect("probe window");
        assert_eq!(probed.candidates.len(), 1);
        assert!(window.has_more());
    }

    fn exact_dense_lane() -> DenseLaneContractV1 {
        DenseLaneContractV1 {
            index: DenseIndexV1::Exact,
            attestation: DenseLaneAttestationV1::Sealed,
        }
    }

    // CASE-COVERS: hybrid explanation honesty over two independent lanes.
    #[test]
    fn build_hybrid_response_explanation_reports_honest_lane_contribution_v1() {
        use super::build_hybrid_response_explanation;
        use quanta_index_contract::EngineTouched;
        // args: (lexical_hits, semantic_hits, fused_universe, fused, top_k, stop, dense lane)
        // Both lanes contributed -> genuine RRF over both engines, and the
        // trace says the lanes are independent (QI-BB-018).
        let both = build_hybrid_response_explanation(2, 2, 3, 2, 100, None, &exact_dense_lane());
        assert_eq!(both.strategy, "rrf", "both-lane hybrid must stay rrf");
        assert_eq!(
            both.engines_touched,
            vec![EngineTouched::Lexical, EngineTouched::Semantic],
            "both-lane hybrid must report symmetric rrf over both engines"
        );
        assert!(
            both.planner_trace.iter().any(|e| e.detail
                == "hybrid.lanes=independent; lexical_hits=2; semantic_hits=2; fused_universe=3"),
            "hybrid trace must expose the independent lanes and the fused universe: {:?}",
            both.planner_trace
        );

        // Lexical found candidates but the dense lane matched none -> must
        // NOT claim a symmetric rrf fusion; it is lexical-only and the
        // Semantic engine is not touched.
        let lex_only =
            build_hybrid_response_explanation(3, 0, 3, 3, 100, None, &exact_dense_lane());
        assert_eq!(
            lex_only.strategy, "lexical_only",
            "semantic-empty hybrid must report lexical_only, not rrf"
        );
        assert_eq!(
            lex_only.engines_touched,
            vec![EngineTouched::Lexical],
            "semantic-empty hybrid must not over-claim the Semantic engine"
        );

        // No lane found anything: honest "empty", no engines claimed.
        let empty = build_hybrid_response_explanation(0, 0, 0, 0, 100, None, &exact_dense_lane());
        assert_eq!(empty.strategy, "empty", "no-hit hybrid must report empty");
        assert!(
            empty.engines_touched.is_empty(),
            "empty hybrid must claim no engines, got {:?}",
            empty.engines_touched
        );

        // Dense-only recall is a real outcome now: the lexical lane found
        // nothing but the dense lane did.
        let semantic_only =
            build_hybrid_response_explanation(0, 1, 1, 1, 100, None, &exact_dense_lane());
        assert_eq!(semantic_only.strategy, "semantic_only");
        assert_eq!(semantic_only.engines_touched, vec![EngineTouched::Semantic]);
    }

    // CASE-COVERS: query-time semantic model-identity enforcement (SEM_MODEL_MISMATCH).
    #[test]
    fn ensure_query_model_matches_index_v1_fails_closed_on_model_drift() {
        use super::ensure_query_model_matches_index_v1;
        use quanta_index_contract::lex::LexicalErrorCode;
        use quanta_index_core::CoreError;

        let expect_model_mismatch = |err: CoreError| {
            #[expect(
                clippy::wildcard_enum_match_arm,
                reason = "the test intentionally rejects every non-typed model-mismatch error"
            )]
            match err {
                CoreError::Typed { code, .. } => assert_eq!(
                    code,
                    LexicalErrorCode::SemModelMismatch.as_code_str(),
                    "model drift must surface SEM_MODEL_MISMATCH"
                ),
                other => panic!("expected SemModelMismatch typed error, got {other:?}"),
            }
        };

        // POSITIVE: identical model id + revision => Ok (matching path proceeds).
        assert!(
            ensure_query_model_matches_index_v1(
                "search-owned-hash-text-v1",
                "r1",
                "search-owned-hash-text-v1",
                Some("r1"),
                "semantic",
            )
            .is_ok()
        );
        assert!(ensure_query_model_matches_index_v1("m", "2", "m", Some("2"), "hybrid").is_ok());

        // ORIGINAL TRIGGER: same dimension is irrelevant — a different model id
        // (the future same-dim engine swap) MUST fail closed, not silently rank.
        expect_model_mismatch(
            ensure_query_model_matches_index_v1(
                "neural-768-v2",
                "r1",
                "search-owned-hash-text-v1",
                Some("r1"),
                "semantic",
            )
            .unwrap_err(),
        );

        // EDGE: same id, revision drift must also fail closed (QI-BB-028).
        expect_model_mismatch(
            ensure_query_model_matches_index_v1("m", "1", "m", Some("2"), "hybrid seed")
                .unwrap_err(),
        );

        // CORNER: an index sealed without a revision cannot be compared and
        // is refused rather than assumed to match.
        expect_model_mismatch(
            ensure_query_model_matches_index_v1("m", "1", "m", None, "semantic").unwrap_err(),
        );
    }
    use quanta_index_contract::CandidatePresenceV1;
    use quanta_index_contract::channel::{LexicalChannelOp, UpsertChunk};
    use quanta_index_contract::lex::{
        CommitRecord, CommitSha, DirtyRecord, LanguageCode, SymbolKindCode, SymbolKindFamily,
    };
    use quanta_index_contract::{
        ChunkId, ChunkRecord, DirtyIngestBatch, DirtyMutation, GenerationPin, GenerationSelector,
        HistoryQueryRequest, HybridQueryRequest, LQ_VERSION_TAG, LexicalCandidate, LqExpr,
        LqFilter, LqLeaf, LqOptions, LqPredicateArg, LqQuery, LqSpan, LqType, ManifestGeneration,
        RepoId, RepoMapDocType, RepoMapEntryDto, RepoMapExactnessSummary,
        RepoMapGraphCoverageClass, RepoMapItemIndexAvailability, RepoMapQueryRequest,
        RepoMapQueryResponse, RepoMapRedactionState, RepoMapSnapshotMeta, RepoRelativePath,
        RevisionId, RuntimeCatalogIngestBatch, RuntimeChangedRecord, RuntimeDocFacetRecord,
        RuntimeEdgeAuthorityRecord, RuntimeMetadataQueryRequest, RuntimeSnapshotRecord,
        SearchPlaneQueryIpcRequest, SearchPlaneQueryIpcResponse, SemanticCorpusKindV1,
        SemanticQueryRequest, SemanticSeedCorpusBudgetV1, SymbolCandidate, TextQueryRequest,
        TextQuerySyntax, UpsertCommit,
    };
    use quanta_index_core::{
        CoreError, LexicalCandidateExplanationV1, LexicalIndexOpenPort, LexicalScoreEngineV1,
        LexicalSearcher, RepoMapQueryPort, SemanticIndexOpenPort, SemanticSearcher,
    };
    use quanta_index_lq_bridge::BridgeErrorCode;
    use quanta_index_lq_obs::{Dimensions, MetricKind, MetricSample};
    use tempfile::tempdir;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn encode_cbor<T: serde::Serialize>(value: &T) -> Result<Vec<u8>, quanta_index_ipc::IpcError> {
        quanta_index_ipc::encode_cbor_payload(value)
    }

    fn default_query_embedder() -> Arc<dyn QueryTextEmbedderPort + Send + Sync> {
        Arc::new(HashingQueryTextEmbedder::new(
            SEARCH_OWNED_SEMANTIC_DIMENSION,
        ))
    }

    struct RejectLexicalOpener;

    impl LexicalIndexOpenPort for RejectLexicalOpener {
        fn open(
            &self,
            _repo: &RepoId,
            _revision: &RevisionId,
            _generation: ManifestGeneration,
        ) -> Result<Box<dyn LexicalSearcher>, CoreError> {
            Err(CoreError::NotImplemented(
                "repo-map dispatch should not open lexical index".to_string(),
            ))
        }
    }

    struct RejectSemanticOpener;

    impl SemanticIndexOpenPort for RejectSemanticOpener {
        fn open(
            &self,
            _repo: &RepoId,
            _revision: &RevisionId,
            _generation: ManifestGeneration,
        ) -> Result<Box<dyn SemanticSearcher>, CoreError> {
            Err(CoreError::NotImplemented(
                "repo-map dispatch should not open semantic index".to_string(),
            ))
        }
    }

    struct StubRepoMapQueryPort;

    impl RepoMapQueryPort for StubRepoMapQueryPort {
        fn query(&self, request: RepoMapQueryRequest) -> Result<RepoMapQueryResponse, CoreError> {
            Ok(RepoMapQueryResponse {
                repo_id: request.repo_id,
                revision_id: request.revision_id,
                manifest_generation: request.manifest_generation,
                snapshot_meta: RepoMapSnapshotMeta {
                    snapshot_id: "dispatch-snapshot".to_string(),
                    projection_version: 1,
                    authority_digest: "dispatch-digest".to_string(),
                    item_index_availability: RepoMapItemIndexAvailability::Available,
                    graph_coverage_class: RepoMapGraphCoverageClass::Full,
                    exactness_summary: RepoMapExactnessSummary::Exact,
                },
                entries: vec![RepoMapEntryDto {
                    subject_identity: "src/lib.rs::Owner".to_string(),
                    subject_doc_type: RepoMapDocType::Symbol,
                    subject_kind: "symbol".to_string(),
                    owner_path: "src/lib.rs".to_string(),
                    score: 1.0,
                    final_score_millis: 1000,
                    rank: 1,
                    importance_score_millis: 900,
                    utility_score_millis: 700,
                    freshness_score_millis: 600,
                    evidence_priority_millis: 500,
                    token_budget_hint: 64,
                    contributing_signals: std::collections::BTreeMap::new(),
                    projection_evidence_kind: "ParserItemIndex".to_string(),
                    projection_authority_artifact_id: "repo-map:dispatch:1".to_string(),
                    projection_authority_digest: "d".repeat(64),
                    projection_status: "Complete".to_string(),
                    redaction_state: RepoMapRedactionState::Unredacted,
                }],
                dropped_entries_count: 0,
                drop_reason_codes: Vec::new(),
                degraded_reason_codes: Vec::new(),
            })
        }
    }

    fn repo_map_request() -> RepoMapQueryRequest {
        RepoMapQueryRequest {
            repo_id: RepoId::new("repo-map-ipc"),
            revision_id: RevisionId::new("rev-map-ipc"),
            manifest_generation: ManifestGeneration::new(9),
            query_text: "dispatch owner".to_string(),
            top_k: 4,
            token_budget: 256,
            focus_subjects: vec![quanta_index_contract::RepoMapFocusSubjectDto {
                subject_identity: "src/lib.rs::Owner".to_string(),
                subject_doc_type: RepoMapDocType::Symbol,
            }],
        }
    }

    fn into_repo_map_query_response(
        response: SearchPlaneQueryIpcResponse,
    ) -> Result<RepoMapQueryResponse, Box<dyn std::error::Error>> {
        match response {
            SearchPlaneQueryIpcResponse::RepoMapQuery(response) => Ok(response),
            other @ (SearchPlaneQueryIpcResponse::Text(_)
            | SearchPlaneQueryIpcResponse::Symbol(_)
            | SearchPlaneQueryIpcResponse::Semantic(_)
            | SearchPlaneQueryIpcResponse::Hybrid(_)
            | SearchPlaneQueryIpcResponse::HybridSeed(_)
            | SearchPlaneQueryIpcResponse::History(_)
            | SearchPlaneQueryIpcResponse::Structural(_)
            | SearchPlaneQueryIpcResponse::Explain(_)
            | SearchPlaneQueryIpcResponse::Error(_)
            | quanta_index_contract::SearchPlaneQueryIpcResponse::ClusterMembershipRead(
                _,
            )
            | quanta_index_contract::SearchPlaneQueryIpcResponse::RuntimeMetadata(_)) => {
                Err(format!("expected repo-map query response, got {other:?}").into())
            }
        }
    }

    fn test_activation_catalog() -> Result<Arc<ActivationCatalog>, Box<dyn std::error::Error>> {
        let dir = tempdir()?;
        Ok(Arc::new(ActivationCatalog::open(dir.keep())?))
    }

    fn corpus_generation(
        repo_id: RepoId,
        revision_id: RevisionId,
        manifest_generation: ManifestGeneration,
        manifest_digest: &str,
    ) -> Result<SearchCorpusGenerationV1, quanta_index_core::CoreError> {
        SearchCorpusGenerationV1::new(
            quanta_index_contract::GenerationSnapshot {
                repo_id: repo_id.clone(),
                revision_id: revision_id.clone(),
                track: SearchPlaneTrackKind::Lexical,
                manifest_generation,
                manifest_digest: manifest_digest.to_string(),
            },
            quanta_index_contract::GenerationSnapshot {
                repo_id,
                revision_id,
                track: SearchPlaneTrackKind::Semantic,
                manifest_generation,
                manifest_digest: manifest_digest.to_string(),
            },
        )
    }

    fn activation_catalog_with_generations(
        generations: &[SearchCorpusGenerationV1],
    ) -> Result<Arc<ActivationCatalog>, Box<dyn std::error::Error>> {
        let catalog = test_activation_catalog()?;
        for generation in generations {
            let prepared = PreparedSearchCorpusGenerationV1::new(generation.clone(), None)?;
            let activation = catalog.activate_prepared_search_corpus_generation_v1(&prepared)?;
            if activation.active != *generation {
                return Err(
                    "activation receipt did not preserve the prepared composite generation".into(),
                );
            }
        }
        Ok(catalog)
    }

    fn ready_ledger() -> Arc<RwLock<Ledger>> {
        let mut ledger = Ledger::default();
        let repo_id = RepoId::new("repo-map-ipc");
        let revision_id = RevisionId::new("rev-map-ipc");
        ledger.lexical_seal(ManifestGeneration::new(9));
        ledger.semantic_seal_with_digest(ManifestGeneration::new(9), "manifest-digest-9");
        ledger.record_track_materialized(
            &repo_id,
            &revision_id,
            SearchPlaneTrackKind::Lexical,
            ManifestGeneration::new(9),
            None,
        );
        ledger.record_track_seal(
            &repo_id,
            &revision_id,
            SearchPlaneTrackKind::Lexical,
            ManifestGeneration::new(9),
        );
        ledger.record_track_materialized(
            &repo_id,
            &revision_id,
            SearchPlaneTrackKind::Semantic,
            ManifestGeneration::new(9),
            Some("manifest-digest-9"),
        );
        ledger.record_track_seal_with_digest(
            &repo_id,
            &revision_id,
            SearchPlaneTrackKind::Semantic,
            ManifestGeneration::new(9),
            "manifest-digest-9",
        );
        Arc::new(RwLock::new(ledger))
    }

    fn cluster_membership_batch_request_v1() -> ClusterMembershipBatchReadRequestV1 {
        ClusterMembershipBatchReadRequestV1 {
            generation: ready_pin(),
            items: vec![
                quanta_index_contract::ClusterMembershipBatchReadItemV1 {
                    cluster_record_id: "cluster-card:auth".to_string(),
                    expected_authority_digest: "authority:auth".to_string(),
                    limit: 2,
                },
                quanta_index_contract::ClusterMembershipBatchReadItemV1 {
                    cluster_record_id: "cluster-card:billing".to_string(),
                    expected_authority_digest: "authority:billing".to_string(),
                    limit: 2,
                },
            ],
        }
    }

    fn available_cluster_membership_batch_response_v1(
        request: &ClusterMembershipBatchReadRequestV1,
    ) -> ClusterMembershipBatchReadResponseV1 {
        ClusterMembershipBatchReadResponseV1 {
            outcomes: request
                .items
                .iter()
                .map(|item| {
                    quanta_index_contract::ClusterMembershipReadOutcomeV1::Available(
                        quanta_index_contract::ClusterMembershipSnapshotV1 {
                            cluster_record_id: item.cluster_record_id.clone(),
                            generation: request.generation.clone(),
                            authority_digest: item.expected_authority_digest.clone(),
                            members: vec![quanta_index_contract::SymbolId::new(format!(
                                "symbol:{}",
                                item.cluster_record_id
                            ))],
                            completeness:
                                quanta_index_contract::ClusterMembershipCompletenessV1::Complete,
                        },
                    )
                })
                .collect(),
        }
    }

    fn history_commit_sha() -> CommitSha {
        CommitSha::from_bytes([
            0x10, 0x32, 0x54, 0x76, 0x98, 0xba, 0xdc, 0xfe, 0x10, 0x32, 0x54, 0x76, 0x98, 0xba,
            0xdc, 0xfe, 0x10, 0x32, 0x54, 0x76,
        ])
    }

    fn rev_at_time_ancestor_sha() -> CommitSha {
        CommitSha::from_hex("1111111111111111111111111111111111111111")
            .expect("valid ancestor commit sha hex")
    }

    fn rev_at_time_head_sha() -> CommitSha {
        CommitSha::from_hex("2222222222222222222222222222222222222222")
            .expect("valid head commit sha hex")
    }

    fn history_commit_record() -> CommitRecord {
        CommitRecord {
            wire_version: 1,
            sha: history_commit_sha(),
            parents: Vec::new(),
            author_time_ms: 1,
            committer_time_ms: 2,
            applied_at_ms: 3,
            author: "alice".to_string().into_boxed_str(),
            author_name: None,
            author_email: None,
            committer: "alice".to_string().into_boxed_str(),
            committer_name: None,
            committer_email: None,
            message: "fix: history lane".to_string().into_boxed_str(),
            is_merge: false,
            tags: Vec::new(),
        }
    }

    fn history_query_request(query_text: &str) -> SearchPlaneQueryIpcRequest {
        SearchPlaneQueryIpcRequest::History(HistoryQueryRequest {
            text_query: TextQueryRequest {
                syntax: TextQuerySyntax::Native,
                query_text: query_text.to_string(),
                constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                generation: Some(ready_pin()),
                generation_selector: None,
                top_k: 5,
            },
            cursor: None,
        })
    }

    fn manual_query(expr: LqExpr, filters: Vec<LqFilter>) -> LqQuery {
        LqQuery {
            lq_version: LQ_VERSION_TAG,
            expr,
            filters,
            directives: Vec::new(),
            options: LqOptions::defaults(),
            source_span: LqSpan::synthetic(0),
        }
    }

    fn history_dispatcher_with_ledger(
        ledger: Arc<RwLock<Ledger>>,
    ) -> Result<SearchPlaneDispatcher, Box<dyn std::error::Error>> {
        Ok(SearchPlaneDispatcher::new(
            Arc::new(RejectLexicalOpener),
            Arc::new(RejectSemanticOpener),
            Arc::new(StubRepoMapQueryPort),
            Arc::new(FailClosedStructuralProducer),
            ledger,
            test_activation_catalog()?,
        ))
    }

    fn runtime_metadata_dispatcher_with_ledger(
        ledger: Arc<RwLock<Ledger>>,
    ) -> Result<SearchPlaneDispatcher, Box<dyn std::error::Error>> {
        Ok(SearchPlaneDispatcher::new(
            Arc::new(RejectLexicalOpener),
            Arc::new(RejectSemanticOpener),
            Arc::new(StubRepoMapQueryPort),
            Arc::new(FailClosedStructuralProducer),
            ledger,
            test_activation_catalog()?,
        ))
    }

    fn runtime_query_request(
        syntax: TextQuerySyntax,
        query_text: &str,
    ) -> SearchPlaneQueryIpcRequest {
        SearchPlaneQueryIpcRequest::RuntimeMetadata(RuntimeMetadataQueryRequest {
            text_query: TextQueryRequest {
                syntax,
                query_text: query_text.to_string(),
                constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                generation: Some(ready_pin()),
                generation_selector: None,
                top_k: 5,
            },
        })
    }

    fn ledger_with_history_ops(
        ops: Vec<LexicalChannelOp>,
    ) -> Result<Arc<RwLock<Ledger>>, Box<dyn std::error::Error>> {
        let ledger = ready_ledger();
        {
            let mut guard = ledger
                .write()
                .map_err(|err| format!("history test ledger poisoned: {err}"))?;
            for op in ops {
                guard.apply_lexical_authority_op(&op)?;
            }
        }
        Ok(ledger)
    }

    fn ledger_with_rev_at_time_history() -> Result<Arc<RwLock<Ledger>>, Box<dyn std::error::Error>>
    {
        let ledger = Arc::new(RwLock::new(Ledger::default()));
        let repo_id = RepoId::new("repo-map-ipc");
        let base_revision_id = RevisionId::new("2222222222222222222222222222222222222222");
        let ancestor_revision_id = RevisionId::new("1111111111111111111111111111111111111111");
        {
            let mut guard = ledger
                .write()
                .map_err(|err| format!("rev_at_time ledger poisoned: {err}"))?;
            guard.record_track_materialized(
                &repo_id,
                &base_revision_id,
                SearchPlaneTrackKind::Lexical,
                ManifestGeneration::new(9),
                None,
            );
            guard.record_track_seal(
                &repo_id,
                &base_revision_id,
                SearchPlaneTrackKind::Lexical,
                ManifestGeneration::new(9),
            );
            guard.record_track_materialized(
                &repo_id,
                &ancestor_revision_id,
                SearchPlaneTrackKind::Lexical,
                ManifestGeneration::new(7),
                None,
            );
            guard.record_track_seal(
                &repo_id,
                &ancestor_revision_id,
                SearchPlaneTrackKind::Lexical,
                ManifestGeneration::new(7),
            );
            guard.apply_history_batch(&quanta_index_contract::HistoryIngestBatch {
                repo_id,
                revision_id: base_revision_id,
                generation: ManifestGeneration::new(9),
                manifest_digest: Some("history-rev-at-time".to_string()),
                batch_digest: "history-rev-at-time-batch".to_string(),
                commits: vec![
                    CommitRecord {
                        wire_version: 1,
                        sha: rev_at_time_ancestor_sha(),
                        parents: Vec::new(),
                        author_time_ms: 100,
                        committer_time_ms: 100,
                        applied_at_ms: 100,
                        author: "alice".to_string().into_boxed_str(),
                        author_name: None,
                        author_email: None,
                        committer: "alice".to_string().into_boxed_str(),
                        committer_name: None,
                        committer_email: None,
                        message: "old commit".to_string().into_boxed_str(),
                        is_merge: false,
                        tags: Vec::new(),
                    },
                    CommitRecord {
                        wire_version: 1,
                        sha: rev_at_time_head_sha(),
                        parents: vec![rev_at_time_ancestor_sha()],
                        author_time_ms: 200,
                        committer_time_ms: 200,
                        applied_at_ms: 200,
                        author: "alice".to_string().into_boxed_str(),
                        author_name: None,
                        author_email: None,
                        committer: "alice".to_string().into_boxed_str(),
                        committer_name: None,
                        committer_email: None,
                        message: "head commit".to_string().into_boxed_str(),
                        is_merge: false,
                        tags: Vec::new(),
                    },
                ],
                refs: vec![quanta_index_contract::HistoryRefMutation::Upsert(
                    quanta_index_contract::HistoryRefUpsert {
                        name: "HEAD".to_string().into_boxed_str(),
                        sha: rev_at_time_head_sha(),
                    },
                )],
                tags: Vec::new(),
                diff_hunks: Vec::new(),
            })?;
        }
        Ok(ledger)
    }

    fn candidate(id: &str, score: f32) -> LexicalCandidate {
        LexicalCandidate {
            candidate_id: id.to_string(),
            repo_id: RepoId::new("repo-map-ipc"),
            revision_id: RevisionId::new("rev-map-ipc"),
            manifest_generation: ManifestGeneration::new(9),
            repo_relative_path: RepoRelativePath::new("src/lib.rs"),
            start_line: 1,
            end_line: 1,
            score,
            snippet: String::new(),
            snippet_hit_offset: None,
            highlights: Vec::new(),
        }
    }

    fn symbol_candidate(id: &str, score: f32) -> SymbolCandidate {
        SymbolCandidate {
            candidate_id: id.to_string(),
            repo_id: RepoId::new("repo-map-ipc"),
            revision_id: RevisionId::new("rev-map-ipc"),
            manifest_generation: ManifestGeneration::new(9),
            repo_relative_path: RepoRelativePath::new("src/lib.rs"),
            start_line: 1,
            end_line: 1,
            score,
            snippet: "MySymbol crate".to_string(),
            symbol_kind: SymbolKindCode::from_code_str("function")
                .unwrap_or_else(|| std::process::abort()),
            symbol_kind_family: Some(SymbolKindFamily::Callable),
        }
    }

    #[derive(Default)]
    struct RecordingSemanticState {
        search_vectors: Vec<Vec<f32>>,
        search_hit_vectors: Vec<Vec<f32>>,
        corpus_searches: Vec<(SemanticCorpusKindV1, u32)>,
        scoped_vectors: Vec<Vec<f32>>,
        search_constraints: Vec<QueryConstraintSetV1>,
        search_hit_constraints: Vec<QueryConstraintSetV1>,
        corpus_constraints: Vec<QueryConstraintSetV1>,
        scoped_constraints: Vec<QueryConstraintSetV1>,
        cluster_membership_opened_pins: Vec<(RepoId, RevisionId, ManifestGeneration)>,
        cluster_membership_requests: Vec<ClusterMembershipBatchReadRequestV1>,
        cluster_membership_response: Option<ClusterMembershipBatchReadResponseV1>,
    }

    struct RecordingSemanticSearcher {
        state: Arc<Mutex<RecordingSemanticState>>,
    }

    impl SemanticSearcher for RecordingSemanticSearcher {
        fn resident_bytes_estimate(&self) -> u64 {
            0
        }

        fn cluster_membership_batch_read(
            &self,
            request: &ClusterMembershipBatchReadRequestV1,
        ) -> Result<ClusterMembershipBatchReadResponseV1, CoreError> {
            let recorded_response = {
                let mut state = self
                    .state
                    .lock()
                    .map_err(|err| CoreError::Storage(format!("semantic state poisoned: {err}")))?;
                state.cluster_membership_requests.push(request.clone());
                state.cluster_membership_response.clone()
            };
            if let Some(response) = recorded_response {
                return Ok(response);
            }
            Ok(ClusterMembershipBatchReadResponseV1 {
                outcomes: request
                    .items
                    .iter()
                    .map(|item| {
                        quanta_index_contract::ClusterMembershipReadOutcomeV1::Rejected(
                            quanta_index_contract::ClusterMembershipReadRejectionV1 {
                                cluster_record_id: item.cluster_record_id.clone(),
                                generation: request.generation.clone(),
                                expected_authority_digest: item
                                    .expected_authority_digest
                                    .clone(),
                                failure: quanta_index_contract::ClusterMembershipReadFailureV1::CurrentGenerationMissing,
                            },
                        )
                    })
                    .collect(),
            })
        }

        fn search(
            &self,
            query_vector: &[f32],
            _top_k: u32,
        ) -> Result<Vec<LexicalCandidate>, CoreError> {
            self.state
                .lock()
                .map_err(|err| CoreError::Storage(format!("semantic state poisoned: {err}")))?
                .search_vectors
                .push(query_vector.to_vec());
            Ok(vec![candidate("semantic-inline", 1.0)])
        }

        fn search_constrained(
            &self,
            query_vector: &[f32],
            constraints: &QueryConstraintSetV1,
            _top_k: u32,
        ) -> Result<Vec<LexicalCandidate>, CoreError> {
            {
                let mut state = self
                    .state
                    .lock()
                    .map_err(|err| CoreError::Storage(format!("semantic state poisoned: {err}")))?;
                state.search_vectors.push(query_vector.to_vec());
                state.search_constraints.push(constraints.clone());
            }
            Ok(vec![candidate("semantic-inline", 1.0)])
        }

        fn search_hits(
            &self,
            query_vector: &[f32],
            _top_k: u32,
        ) -> Result<Vec<SemanticSearchHitV1>, CoreError> {
            self.state
                .lock()
                .map_err(|err| CoreError::Storage(format!("semantic state poisoned: {err}")))?
                .search_hit_vectors
                .push(query_vector.to_vec());
            Ok(vec![SemanticSearchHitV1 {
                candidate: candidate("semantic-inline", 1.0),
                record_id: "semantic-inline-record".to_string(),
                owner_id: "semantic-inline-owner".to_string(),
                owner_kind: quanta_index_contract::OwnerDocKind::Chunk,
                corpus_kind: None,
                authority_digest: "authority:semantic-inline".to_string(),
            }])
        }

        fn search_hits_constrained(
            &self,
            query_vector: &[f32],
            constraints: &QueryConstraintSetV1,
            _top_k: u32,
        ) -> Result<Vec<SemanticSearchHitV1>, CoreError> {
            {
                let mut state = self
                    .state
                    .lock()
                    .map_err(|err| CoreError::Storage(format!("semantic state poisoned: {err}")))?;
                state.search_hit_vectors.push(query_vector.to_vec());
                state.search_hit_constraints.push(constraints.clone());
            }
            Ok(vec![SemanticSearchHitV1 {
                candidate: candidate("semantic-inline", 1.0),
                record_id: "semantic-inline-record".to_string(),
                owner_id: "semantic-inline-owner".to_string(),
                owner_kind: quanta_index_contract::OwnerDocKind::Chunk,
                corpus_kind: None,
                authority_digest: "authority:semantic-inline".to_string(),
            }])
        }

        fn search_hits_for_corpus(
            &self,
            query_vector: &[f32],
            corpus_kind: SemanticCorpusKindV1,
            top_k: u32,
        ) -> Result<Vec<SemanticSearchHitV1>, CoreError> {
            let mut state = self
                .state
                .lock()
                .map_err(|err| CoreError::Storage(format!("semantic state poisoned: {err}")))?;
            state.search_hit_vectors.push(query_vector.to_vec());
            state.corpus_searches.push((corpus_kind, top_k));
            drop(state);
            if corpus_kind == SemanticCorpusKindV1::RepositorySummary {
                return Ok(Vec::new());
            }
            Ok(vec![SemanticSearchHitV1 {
                candidate: candidate(corpus_kind.as_code_str(), 1.0),
                record_id: format!("record:{}", corpus_kind.as_code_str()),
                owner_id: format!("owner:{}", corpus_kind.as_code_str()),
                owner_kind: quanta_index_contract::OwnerDocKind::Symbol,
                corpus_kind: Some(corpus_kind),
                authority_digest: format!("authority:{}", corpus_kind.as_code_str()),
            }])
        }

        fn search_hits_for_corpus_constrained(
            &self,
            query_vector: &[f32],
            corpus_kind: SemanticCorpusKindV1,
            constraints: &QueryConstraintSetV1,
            top_k: u32,
        ) -> Result<Vec<SemanticSearchHitV1>, CoreError> {
            let mut state = self
                .state
                .lock()
                .map_err(|err| CoreError::Storage(format!("semantic state poisoned: {err}")))?;
            state.search_hit_vectors.push(query_vector.to_vec());
            state.corpus_searches.push((corpus_kind, top_k));
            state.corpus_constraints.push(constraints.clone());
            drop(state);
            if corpus_kind == SemanticCorpusKindV1::RepositorySummary {
                return Ok(Vec::new());
            }
            Ok(vec![SemanticSearchHitV1 {
                candidate: candidate(corpus_kind.as_code_str(), 1.0),
                record_id: format!("record:{}", corpus_kind.as_code_str()),
                owner_id: format!("owner:{}", corpus_kind.as_code_str()),
                owner_kind: quanta_index_contract::OwnerDocKind::Symbol,
                corpus_kind: Some(corpus_kind),
                authority_digest: format!("authority:{}", corpus_kind.as_code_str()),
            }])
        }

        fn search_scoped(
            &self,
            query_vector: &[f32],
            _allowed_ids: &std::collections::BTreeSet<String>,
            _top_k: u32,
        ) -> Result<Vec<LexicalCandidate>, CoreError> {
            self.state
                .lock()
                .map_err(|err| CoreError::Storage(format!("semantic state poisoned: {err}")))?
                .scoped_vectors
                .push(query_vector.to_vec());
            Ok(vec![candidate("semantic-scoped", 1.0)])
        }

        fn search_scoped_constrained(
            &self,
            query_vector: &[f32],
            _allowed_ids: &std::collections::BTreeSet<String>,
            constraints: &QueryConstraintSetV1,
            _top_k: u32,
        ) -> Result<Vec<LexicalCandidate>, CoreError> {
            {
                let mut state = self
                    .state
                    .lock()
                    .map_err(|err| CoreError::Storage(format!("semantic state poisoned: {err}")))?;
                state.scoped_vectors.push(query_vector.to_vec());
                state.scoped_constraints.push(constraints.clone());
            }
            Ok(vec![candidate("semantic-scoped", 1.0)])
        }

        fn index_model_id(&self) -> &str {
            // Match the HashingQueryTextEmbedder these tests query with, so the
            // model-identity gate passes on the matching path (the mismatch path
            // is covered by ensure_query_model_matches_index_v1's unit test).
            crate::SEARCH_OWNED_SEMANTIC_MODEL_ID
        }

        fn index_model_revision(&self) -> Option<&str> {
            Some(crate::query_embedder::SEARCH_OWNED_SEMANTIC_MODEL_REVISION)
        }

        fn dense_lane(&self) -> DenseLaneContractV1 {
            DenseLaneContractV1 {
                index: DenseIndexV1::Exact,
                attestation: DenseLaneAttestationV1::Sealed,
            }
        }
    }

    struct RecordingSemanticOpener {
        state: Arc<Mutex<RecordingSemanticState>>,
    }

    impl SemanticIndexOpenPort for RecordingSemanticOpener {
        fn open(
            &self,
            repo: &RepoId,
            revision: &RevisionId,
            generation: ManifestGeneration,
        ) -> Result<Box<dyn SemanticSearcher>, CoreError> {
            self.state
                .lock()
                .map_err(|err| CoreError::Storage(format!("semantic state poisoned: {err}")))?
                .cluster_membership_opened_pins
                .push((repo.clone(), revision.clone(), generation));
            Ok(Box::new(RecordingSemanticSearcher {
                state: Arc::clone(&self.state),
            }))
        }
    }

    #[test]
    fn cluster_membership_dispatch_rejects_invalid_request_before_semantic_open_v1() -> TestResult {
        let state = Arc::new(Mutex::new(RecordingSemanticState::default()));
        let dispatcher = SearchPlaneDispatcher::new(
            Arc::new(RejectLexicalOpener),
            Arc::new(RecordingSemanticOpener {
                state: Arc::clone(&state),
            }),
            Arc::new(StubRepoMapQueryPort),
            Arc::new(FailClosedStructuralProducer),
            ready_ledger(),
            test_activation_catalog()?,
        );
        let request = ClusterMembershipBatchReadRequestV1 {
            generation: ready_pin(),
            items: Vec::new(),
        };

        match dispatcher.cluster_membership_batch_read(&request, &RequestBudgetV1::unbounded()) {
            Err(CoreError::InvalidContract(message)) if message.contains("must not be empty") => {}
            other => {
                return Err(format!("expected invalid-contract rejection, got {other:?}").into());
            }
        }

        let reached_storage = {
            let guard = state
                .lock()
                .map_err(|err| format!("semantic state poisoned: {err}"))?;
            !guard.cluster_membership_opened_pins.is_empty()
                || !guard.cluster_membership_requests.is_empty()
        };
        if reached_storage {
            return Err("invalid membership request reached semantic storage".into());
        }
        Ok(())
    }

    #[test]
    fn cluster_membership_dispatch_opens_one_pinned_generation_and_preserves_authority_v1()
    -> TestResult {
        let request = cluster_membership_batch_request_v1();
        let expected = available_cluster_membership_batch_response_v1(&request);
        let state = Arc::new(Mutex::new(RecordingSemanticState {
            cluster_membership_response: Some(expected.clone()),
            ..RecordingSemanticState::default()
        }));
        let dispatcher = SearchPlaneDispatcher::new(
            Arc::new(RejectLexicalOpener),
            Arc::new(RecordingSemanticOpener {
                state: Arc::clone(&state),
            }),
            Arc::new(StubRepoMapQueryPort),
            Arc::new(FailClosedStructuralProducer),
            ready_ledger(),
            test_activation_catalog()?,
        );

        let observed =
            dispatcher.cluster_membership_batch_read(&request, &RequestBudgetV1::unbounded())?;
        if observed != expected {
            return Err(format!("membership authority drifted: {observed:?}").into());
        }

        let (opened_pins, recorded_requests) = {
            let guard = state
                .lock()
                .map_err(|err| format!("semantic state poisoned: {err}"))?;
            (
                guard.cluster_membership_opened_pins.clone(),
                guard.cluster_membership_requests.clone(),
            )
        };
        if opened_pins.as_slice()
            != [(
                request.generation.repo_id.clone(),
                request.generation.revision_id.clone(),
                request.generation.manifest_generation,
            )]
        {
            return Err(
                format!("membership read must open its exact pin once: {opened_pins:?}").into(),
            );
        }
        if recorded_requests.as_slice() != [request] {
            return Err(format!(
                "membership request must reach the searcher exactly once: {recorded_requests:?}"
            )
            .into());
        }
        Ok(())
    }

    #[test]
    fn cluster_membership_dispatch_rejects_forged_or_stale_searcher_authority_v1() -> TestResult {
        let request = cluster_membership_batch_request_v1();
        let mut forged_response = available_cluster_membership_batch_response_v1(&request);
        let Some(quanta_index_contract::ClusterMembershipReadOutcomeV1::Available(snapshot)) =
            forged_response.outcomes.first_mut()
        else {
            return Err("fixture must contain one available membership".into());
        };
        snapshot.authority_digest = "forged-authority".to_string();

        let mut stale_response = available_cluster_membership_batch_response_v1(&request);
        let Some(quanta_index_contract::ClusterMembershipReadOutcomeV1::Available(snapshot)) =
            stale_response.outcomes.first_mut()
        else {
            return Err("fixture must contain one available membership".into());
        };
        snapshot.generation.manifest_generation = ManifestGeneration::new(8);

        for (case, response) in [("forged", forged_response), ("stale", stale_response)] {
            let state = Arc::new(Mutex::new(RecordingSemanticState {
                cluster_membership_response: Some(response),
                ..RecordingSemanticState::default()
            }));
            let dispatcher = SearchPlaneDispatcher::new(
                Arc::new(RejectLexicalOpener),
                Arc::new(RecordingSemanticOpener {
                    state: Arc::clone(&state),
                }),
                Arc::new(StubRepoMapQueryPort),
                Arc::new(FailClosedStructuralProducer),
                ready_ledger(),
                test_activation_catalog()?,
            );

            match dispatcher.cluster_membership_batch_read(&request, &RequestBudgetV1::unbounded())
            {
                Err(CoreError::InvalidContract(message))
                    if message.contains("invalid authority") => {}
                other => {
                    return Err(format!(
                        "{case} membership authority must fail InvalidContract, got {other:?}"
                    )
                    .into());
                }
            }
            let guard = state
                .lock()
                .map_err(|err| format!("semantic state poisoned: {err}"))?;
            if guard.cluster_membership_opened_pins.len() != 1
                || guard.cluster_membership_requests.as_slice() != [request.clone()]
            {
                return Err(format!(
                    "{case} authority must be rejected after exactly one storage read"
                )
                .into());
            }
        }
        Ok(())
    }

    struct StubLexicalSearcher {
        results: Vec<LexicalCandidate>,
    }

    impl LexicalSearcher for StubLexicalSearcher {
        fn resident_bytes_estimate(&self) -> u64 {
            0
        }

        fn search_constrained(
            &self,
            _query: &quanta_index_contract::LqQuery,
            _constraints: &QueryConstraintSetV1,
            _top_k: u32,
        ) -> Result<LexicalSearchPageV1, CoreError> {
            Ok(LexicalSearchPageV1 {
                candidates: self.results.clone(),
                exact_total: None,
            })
        }

        fn project_file_owners(
            &self,
            candidates: &[LexicalCandidate],
        ) -> Result<Vec<quanta_index_contract::FileOwnerProjectionRow>, CoreError> {
            Ok(candidates
                .iter()
                .map(|candidate| quanta_index_contract::FileOwnerProjectionRow {
                    candidate_id: candidate.candidate_id.clone(),
                    repo_id: candidate.repo_id.clone(),
                    revision_id: candidate.revision_id.clone(),
                    manifest_generation: candidate.manifest_generation,
                    repo_relative_path: candidate.repo_relative_path.clone(),
                    owners: Vec::new(),
                })
                .collect())
        }

        fn search_symbols(
            &self,
            _query: &quanta_index_contract::LqQuery,
            _top_k: u32,
        ) -> Result<Vec<SymbolCandidate>, CoreError> {
            Ok(self
                .results
                .iter()
                .map(|candidate| symbol_candidate(candidate.candidate_id.as_str(), candidate.score))
                .collect())
        }

        fn search_all(
            &self,
            _query: &quanta_index_contract::LqQuery,
        ) -> Result<Vec<LexicalCandidate>, CoreError> {
            Ok(self.results.clone())
        }

        fn candidate_presence(&self, candidate_id: &str) -> Result<CandidatePresenceV1, CoreError> {
            Ok(
                if self
                    .results
                    .iter()
                    .any(|candidate| candidate.candidate_id == candidate_id)
                {
                    CandidatePresenceV1::Indexed
                } else {
                    CandidatePresenceV1::NotIndexed
                },
            )
        }

        fn explain_candidate(
            &self,
            _query: &quanta_index_contract::LqQuery,
            _constraints: &QueryConstraintSetV1,
            candidate_id: &str,
        ) -> Result<LexicalCandidateExplanationV1, CoreError> {
            // The double scores every stub result at its carried score under
            // a unit boost; the boost arithmetic is the real adapter's to
            // prove.
            Ok(self
                .results
                .iter()
                .find(|candidate| candidate.candidate_id == candidate_id)
                .map_or(LexicalCandidateExplanationV1::NotIndexed, |candidate| {
                    LexicalCandidateExplanationV1::Matched(quanta_index_core::LexicalScoreTraceV1 {
                        engine: LexicalScoreEngineV1::Bm25,
                        engine_score: candidate.score,
                        boost_factor: 1.0,
                        emitted_score: candidate.score,
                    })
                }))
        }
    }

    struct StubLexicalOpener {
        results: Vec<LexicalCandidate>,
    }

    impl LexicalIndexOpenPort for StubLexicalOpener {
        fn open(
            &self,
            _repo: &RepoId,
            _revision: &RevisionId,
            _generation: ManifestGeneration,
        ) -> Result<Box<dyn LexicalSearcher>, CoreError> {
            Ok(Box::new(StubLexicalSearcher {
                results: self.results.clone(),
            }))
        }
    }

    #[derive(Default)]
    struct RecordingLexicalState {
        search_top_ks: Vec<u32>,
        symbol_top_ks: Vec<u32>,
        opened_pins: Vec<(RepoId, RevisionId, ManifestGeneration)>,
        searched_queries: Vec<LqQuery>,
        searched_constraints: Vec<QueryConstraintSetV1>,
        symbol_constraints: Vec<QueryConstraintSetV1>,
    }

    struct RecordingLexicalSearcher {
        state: Arc<Mutex<RecordingLexicalState>>,
        results: Vec<LexicalCandidate>,
    }

    impl LexicalSearcher for RecordingLexicalSearcher {
        fn resident_bytes_estimate(&self) -> u64 {
            0
        }

        fn search_constrained(
            &self,
            query: &quanta_index_contract::LqQuery,
            constraints: &QueryConstraintSetV1,
            top_k: u32,
        ) -> Result<LexicalSearchPageV1, CoreError> {
            let mut guard = self
                .state
                .lock()
                .map_err(|err| CoreError::Storage(format!("lexical state poisoned: {err}")))?;
            guard.search_top_ks.push(top_k);
            guard.searched_queries.push(query.clone());
            guard.searched_constraints.push(constraints.clone());
            drop(guard);
            Ok(LexicalSearchPageV1 {
                candidates: self.results.clone(),
                exact_total: None,
            })
        }

        fn project_file_owners(
            &self,
            candidates: &[LexicalCandidate],
        ) -> Result<Vec<quanta_index_contract::FileOwnerProjectionRow>, CoreError> {
            Ok(candidates
                .iter()
                .map(|candidate| quanta_index_contract::FileOwnerProjectionRow {
                    candidate_id: candidate.candidate_id.clone(),
                    repo_id: candidate.repo_id.clone(),
                    revision_id: candidate.revision_id.clone(),
                    manifest_generation: candidate.manifest_generation,
                    repo_relative_path: candidate.repo_relative_path.clone(),
                    owners: Vec::new(),
                })
                .collect())
        }

        fn search_symbols(
            &self,
            _query: &quanta_index_contract::LqQuery,
            top_k: u32,
        ) -> Result<Vec<SymbolCandidate>, CoreError> {
            self.state
                .lock()
                .map_err(|err| CoreError::Storage(format!("lexical state poisoned: {err}")))?
                .symbol_top_ks
                .push(top_k);
            Ok(self
                .results
                .iter()
                .map(|candidate| symbol_candidate(candidate.candidate_id.as_str(), candidate.score))
                .collect())
        }

        fn search_symbols_constrained(
            &self,
            _query: &quanta_index_contract::LqQuery,
            constraints: &QueryConstraintSetV1,
            top_k: u32,
        ) -> Result<Vec<SymbolCandidate>, CoreError> {
            let mut guard = self
                .state
                .lock()
                .map_err(|err| CoreError::Storage(format!("lexical state poisoned: {err}")))?;
            guard.symbol_top_ks.push(top_k);
            guard.symbol_constraints.push(constraints.clone());
            drop(guard);
            Ok(self
                .results
                .iter()
                .map(|candidate| symbol_candidate(candidate.candidate_id.as_str(), candidate.score))
                .collect())
        }

        fn search_all(
            &self,
            _query: &quanta_index_contract::LqQuery,
        ) -> Result<Vec<LexicalCandidate>, CoreError> {
            Ok(self.results.clone())
        }

        fn candidate_presence(&self, candidate_id: &str) -> Result<CandidatePresenceV1, CoreError> {
            Ok(
                if self
                    .results
                    .iter()
                    .any(|candidate| candidate.candidate_id == candidate_id)
                {
                    CandidatePresenceV1::Indexed
                } else {
                    CandidatePresenceV1::NotIndexed
                },
            )
        }

        fn explain_candidate(
            &self,
            _query: &quanta_index_contract::LqQuery,
            _constraints: &QueryConstraintSetV1,
            candidate_id: &str,
        ) -> Result<LexicalCandidateExplanationV1, CoreError> {
            // The double scores every stub result at its carried score under
            // a unit boost; the boost arithmetic is the real adapter's to
            // prove.
            Ok(self
                .results
                .iter()
                .find(|candidate| candidate.candidate_id == candidate_id)
                .map_or(LexicalCandidateExplanationV1::NotIndexed, |candidate| {
                    LexicalCandidateExplanationV1::Matched(quanta_index_core::LexicalScoreTraceV1 {
                        engine: LexicalScoreEngineV1::Bm25,
                        engine_score: candidate.score,
                        boost_factor: 1.0,
                        emitted_score: candidate.score,
                    })
                }))
        }
    }

    struct RecordingLexicalOpener {
        state: Arc<Mutex<RecordingLexicalState>>,
        results: Vec<LexicalCandidate>,
    }

    impl LexicalIndexOpenPort for RecordingLexicalOpener {
        fn open(
            &self,
            repo: &RepoId,
            revision: &RevisionId,
            generation: ManifestGeneration,
        ) -> Result<Box<dyn LexicalSearcher>, CoreError> {
            self.state
                .lock()
                .map_err(|err| CoreError::Storage(format!("lexical state poisoned: {err}")))?
                .opened_pins
                .push((repo.clone(), revision.clone(), generation));
            Ok(Box::new(RecordingLexicalSearcher {
                state: Arc::clone(&self.state),
                results: self.results.clone(),
            }))
        }
    }

    #[test]
    fn repo_map_dispatcher_branch_delegates_to_repo_map_query_port() -> TestResult {
        let dispatcher = SearchPlaneDispatcher::new(
            Arc::new(RejectLexicalOpener),
            Arc::new(RejectSemanticOpener),
            Arc::new(StubRepoMapQueryPort),
            Arc::new(FailClosedStructuralProducer),
            Arc::new(RwLock::new(Ledger::default())),
            test_activation_catalog()?,
        );

        let response = into_repo_map_query_response(dispatcher.dispatch(
            SearchPlaneQueryIpcRequest::RepoMapQuery(repo_map_request()),
            &RequestBudgetV1::unbounded(),
        ))?;

        if response.repo_id.as_str() != "repo-map-ipc" {
            return Err(format!("unexpected repo id: {}", response.repo_id.as_str()).into());
        }
        if response.revision_id.as_str() != "rev-map-ipc" {
            return Err(
                format!("unexpected revision id: {}", response.revision_id.as_str()).into(),
            );
        }
        if response.manifest_generation.get() != 9 {
            return Err(format!(
                "unexpected manifest generation: {}",
                response.manifest_generation.get()
            )
            .into());
        }
        if response.snapshot_meta.snapshot_id != "dispatch-snapshot" {
            return Err(format!(
                "unexpected snapshot id: {}",
                response.snapshot_meta.snapshot_id
            )
            .into());
        }
        if response.entries.len() != 1 {
            return Err(format!("unexpected entry count: {}", response.entries.len()).into());
        }
        let first_entry = response
            .entries
            .first()
            .ok_or_else(|| "expected one repo-map entry".to_string())?;
        if first_entry.owner_path != "src/lib.rs" {
            return Err(format!("unexpected owner path: {}", first_entry.owner_path).into());
        }
        Ok(())
    }

    #[test]
    fn lexical_dispatch_fail_closed_when_generation_is_not_ready() -> TestResult {
        let dispatcher = SearchPlaneDispatcher::new(
            Arc::new(RejectLexicalOpener),
            Arc::new(RejectSemanticOpener),
            Arc::new(StubRepoMapQueryPort),
            Arc::new(FailClosedStructuralProducer),
            Arc::new(RwLock::new(Ledger::default())),
            test_activation_catalog()?,
        );

        match dispatcher.dispatch(
            SearchPlaneQueryIpcRequest::Text(TextQueryRequest {
                syntax: TextQuerySyntax::Native,
                query_text: "needle".to_string(),
                constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                generation: Some(make_pin(
                    RepoId::new("repo-map-ipc"),
                    RevisionId::new("rev-map-ipc"),
                    ManifestGeneration::new(9),
                )),
                generation_selector: None,
                top_k: 50,
            }),
            &RequestBudgetV1::unbounded(),
        ) {
            SearchPlaneQueryIpcResponse::Error(err) => {
                if err.code != "NOT_READY" {
                    return Err(format!("unexpected error code: {}", err.code).into());
                }
            }
            other @ (SearchPlaneQueryIpcResponse::Text(_)
            | SearchPlaneQueryIpcResponse::Symbol(_)
            | SearchPlaneQueryIpcResponse::Semantic(_)
            | SearchPlaneQueryIpcResponse::Hybrid(_)
            | SearchPlaneQueryIpcResponse::HybridSeed(_)
            | SearchPlaneQueryIpcResponse::History(_)
            | SearchPlaneQueryIpcResponse::Structural(_)
            | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
            | SearchPlaneQueryIpcResponse::Explain(_)
            | quanta_index_contract::SearchPlaneQueryIpcResponse::ClusterMembershipRead(
                _,
            )
            | quanta_index_contract::SearchPlaneQueryIpcResponse::RuntimeMetadata(_)) => {
                return Err(format!("expected Error response, got {other:?}").into());
            }
        }
        Ok(())
    }

    #[test]
    fn sourcegraph_text_syntax_dispatch_returns_text_payload() -> TestResult {
        let state = Arc::new(Mutex::new(RecordingLexicalState::default()));
        let dispatcher = SearchPlaneDispatcher::new(
            Arc::new(RecordingLexicalOpener {
                state: Arc::clone(&state),
                results: vec![candidate("alpha", 1.0), candidate("beta", 0.9)],
            }),
            Arc::new(RejectSemanticOpener),
            Arc::new(StubRepoMapQueryPort),
            Arc::new(FailClosedStructuralProducer),
            ready_ledger(),
            test_activation_catalog()?,
        );

        let pin = make_pin(
            RepoId::new("repo-map-ipc"),
            RevisionId::new("rev-map-ipc"),
            ManifestGeneration::new(9),
        );
        let response = dispatcher.dispatch(
            SearchPlaneQueryIpcRequest::Text(TextQueryRequest {
                syntax: TextQuerySyntax::Sourcegraph,
                query_text: "repo:repo-map-ipc alpha".into(),
                constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                generation: Some(pin.clone()),
                generation_selector: None,
                top_k: 2,
            }),
            &RequestBudgetV1::unbounded(),
        );

        match response {
            SearchPlaneQueryIpcResponse::Text(text) => {
                if text.generation != pin {
                    return Err("text response did not echo request pin".into());
                }
                if text.results.len() != 2 {
                    return Err(
                        format!("expected two text results, got {}", text.results.len()).into(),
                    );
                }
            }
            other @ (SearchPlaneQueryIpcResponse::Symbol(_)
            | SearchPlaneQueryIpcResponse::Semantic(_)
            | SearchPlaneQueryIpcResponse::Hybrid(_)
            | SearchPlaneQueryIpcResponse::HybridSeed(_)
            | SearchPlaneQueryIpcResponse::History(_)
            | SearchPlaneQueryIpcResponse::Structural(_)
            | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
            | SearchPlaneQueryIpcResponse::Explain(_)
            | SearchPlaneQueryIpcResponse::Error(_)
            | SearchPlaneQueryIpcResponse::ClusterMembershipRead(_)
            | SearchPlaneQueryIpcResponse::RuntimeMetadata(_)) => {
                return Err(format!("expected Text response, got {other:?}").into());
            }
        }
        let guard = state
            .lock()
            .map_err(|err| format!("lexical state poisoned: {err}"))?;
        if guard.search_top_ks.as_slice() != [3] {
            return Err(format!(
                "expected sourcegraph route to probe with top_k=3, got {:?}",
                guard.search_top_ks
            )
            .into());
        }
        drop(guard);
        Ok(())
    }

    #[test]
    fn symbol_dispatch_admits_only_typed_exact_path_as_constraint_only_authority_v1() -> TestResult
    {
        let state = Arc::new(Mutex::new(RecordingLexicalState::default()));
        let dispatcher = SearchPlaneDispatcher::new(
            Arc::new(RecordingLexicalOpener {
                state: Arc::clone(&state),
                results: vec![candidate("path-owned", 1.0)],
            }),
            Arc::new(RejectSemanticOpener),
            Arc::new(StubRepoMapQueryPort),
            Arc::new(FailClosedStructuralProducer),
            ready_ledger(),
            test_activation_catalog()?,
        );
        let path =
            quanta_index_contract::ExactRepoRelativePathV1::new("src/a*)' \"literal file.rs")
                .map_err(str::to_string)?;
        let constraints = QueryConstraintSetV1::from_exact_repo_relative_path(path);
        let response = dispatcher.symbol(
            SymbolQueryRequest {
                syntax: TextQuerySyntax::Native,
                query_text: String::new(),
                constraints: constraints.clone(),
                generation: Some(ready_pin()),
                generation_selector: None,
                top_k: 3,
            },
            &RequestBudgetV1::unbounded(),
        )?;
        if response.results.len() != 1 {
            return Err(format!(
                "constraint-only symbol request did not reach the searcher: {:?}",
                response.results
            )
            .into());
        }
        let guard = state
            .lock()
            .map_err(|err| format!("lexical state poisoned: {err}"))?;
        if guard.symbol_constraints.as_slice() != [constraints] {
            return Err(format!(
                "typed exact path was not forwarded verbatim: {:?}",
                guard.symbol_constraints
            )
            .into());
        }
        drop(guard);

        match dispatcher.symbol(
            SymbolQueryRequest {
                syntax: TextQuerySyntax::Native,
                query_text: String::new(),
                constraints: QueryConstraintSetV1::unconstrained(),
                generation: Some(ready_pin()),
                generation_selector: None,
                top_k: 3,
            },
            &RequestBudgetV1::unbounded(),
        ) {
            Err(CoreError::InvalidContract(message))
                if message.contains("empty query is rejected") => {}
            other => {
                return Err(format!(
                    "empty unconstrained symbol request must fail before search: {other:?}"
                )
                .into());
            }
        }
        Ok(())
    }

    #[test]
    fn lexical_dispatch_stabilizes_tied_text_results() -> TestResult {
        let make_candidate = |id: &str, path: &str, start_line: u32, score: f32| LexicalCandidate {
            candidate_id: id.to_string(),
            repo_id: RepoId::new("repo-map-ipc"),
            revision_id: RevisionId::new("rev-map-ipc"),
            manifest_generation: ManifestGeneration::new(9),
            repo_relative_path: RepoRelativePath::new(path),
            start_line,
            end_line: start_line,
            score,
            snippet: String::new(),
            snippet_hit_offset: None,
            highlights: Vec::new(),
        };
        let dispatcher = SearchPlaneDispatcher::new(
            Arc::new(StubLexicalOpener {
                results: vec![
                    make_candidate("z-last", "src/z.rs", 1, 0.5),
                    make_candidate("b-second", "src/b.rs", 1, 1.0),
                    make_candidate("a-third", "src/a.rs", 2, 1.0),
                    make_candidate("a-first", "src/a.rs", 1, 1.0),
                ],
            }),
            Arc::new(RejectSemanticOpener),
            Arc::new(StubRepoMapQueryPort),
            Arc::new(FailClosedStructuralProducer),
            ready_ledger(),
            test_activation_catalog()?,
        );

        let response = dispatcher.dispatch(
            SearchPlaneQueryIpcRequest::Text(TextQueryRequest {
                syntax: TextQuerySyntax::Native,
                query_text: "alpha".into(),
                constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                generation: Some(ready_pin()),
                generation_selector: None,
                top_k: 10,
            }),
            &RequestBudgetV1::unbounded(),
        );

        match response {
            SearchPlaneQueryIpcResponse::Text(text) => {
                let observed: Vec<&str> = text
                    .results
                    .iter()
                    .map(|candidate| candidate.candidate_id.as_str())
                    .collect();
                let expected = vec!["a-first", "a-third", "b-second", "z-last"];
                if observed != expected {
                    return Err(format!(
                        "expected stabilized lexical text order {expected:?}, got {observed:?}"
                    )
                    .into());
                }
            }
            other @ (SearchPlaneQueryIpcResponse::Symbol(_)
            | SearchPlaneQueryIpcResponse::Semantic(_)
            | SearchPlaneQueryIpcResponse::Hybrid(_)
            | SearchPlaneQueryIpcResponse::HybridSeed(_)
            | SearchPlaneQueryIpcResponse::History(_)
            | SearchPlaneQueryIpcResponse::Structural(_)
            | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
            | SearchPlaneQueryIpcResponse::Explain(_)
            | SearchPlaneQueryIpcResponse::Error(_)
            | SearchPlaneQueryIpcResponse::ClusterMembershipRead(_)
            | SearchPlaneQueryIpcResponse::RuntimeMetadata(_)) => {
                return Err(format!("expected Text response, got {other:?}").into());
            }
        }
        Ok(())
    }

    #[test]
    fn sourcegraph_dispatch_rejects_structural_pattern_type_before_lexical_execution() -> TestResult
    {
        let state = Arc::new(Mutex::new(RecordingLexicalState::default()));
        let dispatcher = SearchPlaneDispatcher::new(
            Arc::new(RecordingLexicalOpener {
                state: Arc::clone(&state),
                results: vec![candidate("alpha", 1.0)],
            }),
            Arc::new(RejectSemanticOpener),
            Arc::new(StubRepoMapQueryPort),
            Arc::new(FailClosedStructuralProducer),
            ready_ledger(),
            test_activation_catalog()?,
        );

        let response = dispatcher.dispatch(
            SearchPlaneQueryIpcRequest::Text(TextQueryRequest {
                syntax: TextQuerySyntax::Sourcegraph,
                query_text: r#"patterntype:structural "function_item""#.into(),
                constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                generation: Some(ready_pin()),
                generation_selector: None,
                top_k: 2,
            }),
            &RequestBudgetV1::unbounded(),
        );

        let (code, message) =
            ipc_error_from(response).map_err(Box::<dyn std::error::Error>::from)?;
        if code != BridgeErrorCode::BridgeTranslateFail.as_code_str() {
            return Err(format!(
                "expected BRIDGE_TRANSLATE_FAIL for SG structural lexical route, got {code}"
            )
            .into());
        }
        if !message.contains("use the structural route instead") {
            return Err(format!("unexpected SG structural lexical message: {message}").into());
        }
        let guard = state
            .lock()
            .map_err(|err| format!("lexical state poisoned: {err}"))?;
        if !guard.search_top_ks.is_empty() {
            return Err(
                "lexical opener must not execute for SG structural lexical rejection".into(),
            );
        }
        drop(guard);
        Ok(())
    }

    #[test]
    fn semantic_dispatch_embeds_query_text() -> TestResult {
        let state = Arc::new(Mutex::new(RecordingSemanticState::default()));
        let dispatcher = SearchPlaneDispatcher::new(
            Arc::new(RejectLexicalOpener),
            Arc::new(RecordingSemanticOpener {
                state: Arc::clone(&state),
            }),
            Arc::new(StubRepoMapQueryPort),
            Arc::new(FailClosedStructuralProducer),
            ready_ledger(),
            test_activation_catalog()?,
        );

        let response = dispatcher.dispatch(
            SearchPlaneQueryIpcRequest::Semantic(SemanticQueryRequest {
                query_text: "focus alpha".to_string(),
                constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                generation: Some(GenerationPin::new(
                    RepoId::new("repo-map-ipc"),
                    RevisionId::new("rev-map-ipc"),
                    ManifestGeneration::new(9),
                )),
                generation_selector: None,
                lexical_scope: None,
                top_k: 3,
            }),
            &RequestBudgetV1::unbounded(),
        );

        match response {
            SearchPlaneQueryIpcResponse::Semantic(semantic) => {
                if semantic.results.len() != 1 {
                    return Err(format!(
                        "expected one semantic result, got {}",
                        semantic.results.len()
                    )
                    .into());
                }
            }
            other @ (SearchPlaneQueryIpcResponse::Text(_)
            | SearchPlaneQueryIpcResponse::Symbol(_)
            | SearchPlaneQueryIpcResponse::Hybrid(_)
            | SearchPlaneQueryIpcResponse::HybridSeed(_)
            | SearchPlaneQueryIpcResponse::History(_)
            | SearchPlaneQueryIpcResponse::Structural(_)
            | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
            | SearchPlaneQueryIpcResponse::Explain(_)
            | SearchPlaneQueryIpcResponse::Error(_)
            | quanta_index_contract::SearchPlaneQueryIpcResponse::ClusterMembershipRead(
                _,
            )
            | quanta_index_contract::SearchPlaneQueryIpcResponse::RuntimeMetadata(_)) => {
                return Err(format!("expected Semantic response, got {other:?}").into());
            }
        }

        let (search_vectors, scoped_vectors) = {
            let guard = state
                .lock()
                .map_err(|err| format!("semantic state poisoned: {err}"))?;
            (guard.search_vectors.clone(), guard.scoped_vectors.clone())
        };
        let expected = default_query_embedder().embed_query("focus alpha")?;
        if search_vectors.as_slice() != [expected] {
            return Err(format!("unexpected semantic vectors: {search_vectors:?}").into());
        }
        if !scoped_vectors.is_empty() {
            return Err(format!("unexpected scoped vectors: {scoped_vectors:?}").into());
        }
        Ok(())
    }

    /// Embeds a real vector but advertises a configurable model identity, so a
    /// query-time model drift can be exercised at the dispatcher boundary.
    struct FixedModelQueryEmbedder {
        model_id: &'static str,
        model_revision: &'static str,
        dimension: usize,
    }

    impl QueryTextEmbedderPort for FixedModelQueryEmbedder {
        fn embed_query(&self, query_text: &str) -> Result<Vec<f32>, quanta_index_core::CoreError> {
            HashingQueryTextEmbedder::new(self.dimension).embed_query(query_text)
        }
        fn model_id(&self) -> &'static str {
            self.model_id
        }
        fn model_revision(&self) -> &'static str {
            self.model_revision
        }
    }

    /// Always fails `embed_query` (provider down), to prove the model gate runs
    /// AFTER embed and does not mask the provider-unavailable rail.
    struct UnavailableTestQueryEmbedder;

    impl QueryTextEmbedderPort for UnavailableTestQueryEmbedder {
        fn embed_query(&self, _query_text: &str) -> Result<Vec<f32>, quanta_index_core::CoreError> {
            Err(quanta_index_core::CoreError::Typed {
                code: quanta_index_contract::lex::LexicalErrorCode::SemProviderUnavailable
                    .as_code_str()
                    .to_string(),
                message: "test embedder unavailable".to_string(),
            })
        }
        fn model_id(&self) -> &'static str {
            "provider-unavailable"
        }
        fn model_revision(&self) -> &'static str {
            "unavailable"
        }
    }

    fn semantic_focus_request() -> SemanticQueryRequest {
        SemanticQueryRequest {
            query_text: "focus alpha".to_string(),
            constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
            generation: Some(GenerationPin::new(
                RepoId::new("repo-map-ipc"),
                RevisionId::new("rev-map-ipc"),
                ManifestGeneration::new(9),
            )),
            generation_selector: None,
            lexical_scope: None,
            top_k: 3,
        }
    }

    // CASE-COVERS: query-time model gate WIRING — a model-id drift at the dispatcher
    // boundary fails closed BEFORE the searcher runs (proves the gate is invoked at
    // the call site, not just that the helper logic is correct).
    #[test]
    fn semantic_dispatch_rejects_model_identity_drift_v1() -> TestResult {
        let state = Arc::new(Mutex::new(RecordingSemanticState::default()));
        let dispatcher = SearchPlaneDispatcher::new_with_obs(
            Arc::new(RejectLexicalOpener),
            Arc::new(RecordingSemanticOpener {
                state: Arc::clone(&state),
            }),
            SnapshotRegistries::new(SnapshotRegistryPolicy::DEFAULT),
            Arc::new(StubRepoMapQueryPort),
            Arc::new(FailClosedStructuralProducer),
            ready_ledger(),
            test_activation_catalog()?,
            Arc::new(FixedModelQueryEmbedder {
                model_id: "neural-768-v2",
                model_revision: "r1",
                dimension: SEARCH_OWNED_SEMANTIC_DIMENSION,
            }),
            Arc::new(super::NoopQueryObsSink),
        );

        match dispatcher.semantic(&semantic_focus_request(), &RequestBudgetV1::unbounded()) {
            Err(quanta_index_core::CoreError::Typed { code, .. }) => {
                let expected =
                    quanta_index_contract::lex::LexicalErrorCode::SemModelMismatch.as_code_str();
                if code != expected {
                    return Err(format!("expected SEM_MODEL_MISMATCH, got code {code}").into());
                }
            }
            other => {
                return Err(format!("model drift must fail closed, got {other:?}").into());
            }
        }

        // The gate runs AFTER embed but BEFORE the searcher: no vectors reach the
        // searcher, so a mismatched-model query never produces a (garbage) ranking.
        let guard = state
            .lock()
            .map_err(|err| format!("semantic state poisoned: {err}"))?;
        if !guard.search_vectors.is_empty() || !guard.scoped_vectors.is_empty() {
            return Err(format!(
                "model drift must reject before invoking the searcher; search={:?} scoped={:?}",
                guard.search_vectors, guard.scoped_vectors
            )
            .into());
        }
        drop(guard);
        Ok(())
    }

    // CASE-COVERS: embed-before-model-gate ORDER — an unavailable embedder surfaces
    // SEM_PROVIDER_UNAVAILABLE (from embed), NOT SEM_MODEL_MISMATCH, proving the gate
    // is placed after embed so the provider-unavailable rail keeps its own error.
    #[test]
    fn semantic_dispatch_unavailable_embedder_keeps_provider_error_before_model_gate_v1()
    -> TestResult {
        let state = Arc::new(Mutex::new(RecordingSemanticState::default()));
        let dispatcher = SearchPlaneDispatcher::new_with_obs(
            Arc::new(RejectLexicalOpener),
            Arc::new(RecordingSemanticOpener {
                state: Arc::clone(&state),
            }),
            SnapshotRegistries::new(SnapshotRegistryPolicy::DEFAULT),
            Arc::new(StubRepoMapQueryPort),
            Arc::new(FailClosedStructuralProducer),
            ready_ledger(),
            test_activation_catalog()?,
            Arc::new(UnavailableTestQueryEmbedder),
            Arc::new(super::NoopQueryObsSink),
        );

        match dispatcher.semantic(&semantic_focus_request(), &RequestBudgetV1::unbounded()) {
            Err(quanta_index_core::CoreError::Typed { code, .. }) => {
                let provider = quanta_index_contract::lex::LexicalErrorCode::SemProviderUnavailable
                    .as_code_str();
                if code != provider {
                    return Err(format!(
                        "unavailable embedder must surface SEM_PROVIDER_UNAVAILABLE (embed precedes the model gate), got {code}"
                    )
                    .into());
                }
            }
            other => {
                return Err(format!("unavailable embedder must fail closed, got {other:?}").into());
            }
        }
        Ok(())
    }

    #[test]
    fn hybrid_dispatch_embeds_semantic_query_text() -> TestResult {
        let semantic_state = Arc::new(Mutex::new(RecordingSemanticState::default()));
        let lexical_state = Arc::new(Mutex::new(RecordingLexicalState::default()));
        let constraints = QueryConstraintSetV1::from_exact_repo_relative_path(
            quanta_index_contract::ExactRepoRelativePathV1::new("src/lib.rs")
                .map_err(str::to_string)?,
        );
        let dispatcher = SearchPlaneDispatcher::new(
            Arc::new(RecordingLexicalOpener {
                state: Arc::clone(&lexical_state),
                results: vec![candidate("alpha", 1.0), candidate("beta", 0.9)],
            }),
            Arc::new(RecordingSemanticOpener {
                state: Arc::clone(&semantic_state),
            }),
            Arc::new(StubRepoMapQueryPort),
            Arc::new(FailClosedStructuralProducer),
            ready_ledger(),
            test_activation_catalog()?,
        );

        let pin = make_pin(
            RepoId::new("repo-map-ipc"),
            RevisionId::new("rev-map-ipc"),
            ManifestGeneration::new(9),
        );
        let response = dispatcher.dispatch(
            SearchPlaneQueryIpcRequest::Hybrid(HybridQueryRequest {
                text_query: TextQueryRequest {
                    syntax: TextQuerySyntax::Sourcegraph,
                    query_text: "scope".to_string(),
                    constraints: constraints.clone(),
                    generation: Some(pin.clone()),
                    generation_selector: None,
                    top_k: 2,
                },
                semantic_query_text: "scope alpha".to_string(),
                generation: Some(pin),
                generation_selector: None,
                top_k: 2,
            }),
            &RequestBudgetV1::unbounded(),
        );

        match response {
            SearchPlaneQueryIpcResponse::Hybrid(hybrid) => {
                if hybrid.results.is_empty() {
                    return Err("expected non-empty hybrid results".into());
                }
            }
            other @ (SearchPlaneQueryIpcResponse::HybridSeed(_)
            | SearchPlaneQueryIpcResponse::Text(_)
            | SearchPlaneQueryIpcResponse::Symbol(_)
            | SearchPlaneQueryIpcResponse::Semantic(_)
            | SearchPlaneQueryIpcResponse::History(_)
            | SearchPlaneQueryIpcResponse::Structural(_)
            | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
            | SearchPlaneQueryIpcResponse::Explain(_)
            | SearchPlaneQueryIpcResponse::Error(_)
            | quanta_index_contract::SearchPlaneQueryIpcResponse::ClusterMembershipRead(
                _,
            )
            | quanta_index_contract::SearchPlaneQueryIpcResponse::RuntimeMetadata(_)) => {
                return Err(format!("expected Hybrid response, got {other:?}").into());
            }
        }

        let (scoped_vectors, search_vectors, search_constraints) = {
            let guard = semantic_state
                .lock()
                .map_err(|err| format!("semantic state poisoned: {err}"))?;
            (
                guard.scoped_vectors.clone(),
                guard.search_vectors.clone(),
                guard.search_constraints.clone(),
            )
        };
        let expected = default_query_embedder().embed_query("scope alpha")?;
        // QI-BB-018: the dense lane is independent of the lexical hits — one
        // unscoped search over the query vector, under the request's
        // constraints; never a search scoped to the lexical ids.
        if !scoped_vectors.is_empty() {
            return Err(format!(
                "hybrid must not scope the dense lane to lexical hits: {scoped_vectors:?}"
            )
            .into());
        }
        if search_vectors.as_slice() != [expected] {
            return Err(format!("unexpected dense lane vectors: {search_vectors:?}").into());
        }
        if search_constraints.as_slice() != [constraints.clone()] {
            return Err(format!(
                "hybrid dense lane lost exact-path constraints: {search_constraints:?}"
            )
            .into());
        }
        let searched_constraints = {
            let guard = lexical_state
                .lock()
                .map_err(|err| format!("lexical state poisoned: {err}"))?;
            guard.searched_constraints.clone()
        };
        if searched_constraints.as_slice() != [constraints] {
            return Err(format!(
                "hybrid lexical leg lost exact-path constraints: {searched_constraints:?}"
            )
            .into());
        }
        Ok(())
    }

    #[test]
    fn hybrid_seed_dispatch_includes_dense_only_entity_in_the_seed_set() -> TestResult {
        let semantic_state = Arc::new(Mutex::new(RecordingSemanticState::default()));
        let lexical_state = Arc::new(Mutex::new(RecordingLexicalState::default()));
        let constraints = QueryConstraintSetV1::from_exact_repo_relative_path(
            quanta_index_contract::ExactRepoRelativePathV1::new("src/lib.rs")
                .map_err(str::to_string)?,
        );
        let dispatcher = SearchPlaneDispatcher::new(
            Arc::new(RecordingLexicalOpener {
                state: Arc::clone(&lexical_state),
                results: vec![candidate("alpha", 1.0), candidate("beta", 0.9)],
            }),
            Arc::new(RecordingSemanticOpener {
                state: Arc::clone(&semantic_state),
            }),
            Arc::new(StubRepoMapQueryPort),
            Arc::new(FailClosedStructuralProducer),
            ready_ledger(),
            test_activation_catalog()?,
        );

        let pin = make_pin(
            RepoId::new("repo-map-ipc"),
            RevisionId::new("rev-map-ipc"),
            ManifestGeneration::new(9),
        );
        let response = dispatcher.dispatch(
            SearchPlaneQueryIpcRequest::HybridSeed(HybridSeedQueryRequest {
                text_query: TextQueryRequest {
                    syntax: TextQuerySyntax::Sourcegraph,
                    query_text: "scope".to_string(),
                    constraints: constraints.clone(),
                    generation: Some(pin.clone()),
                    generation_selector: None,
                    top_k: 3,
                },
                semantic_query_text: "scope alpha".to_string(),
                generation: Some(pin),
                generation_selector: None,
                dense_corpora: vec![
                    SemanticSeedCorpusBudgetV1 {
                        corpus_kind: SemanticCorpusKindV1::SymbolCard,
                        top_k: 7,
                    },
                    SemanticSeedCorpusBudgetV1 {
                        corpus_kind: SemanticCorpusKindV1::RepositorySummary,
                        top_k: 11,
                    },
                    SemanticSeedCorpusBudgetV1 {
                        corpus_kind: SemanticCorpusKindV1::ClusterCard,
                        top_k: 13,
                    },
                ],
                top_k: 3,
            }),
            &RequestBudgetV1::unbounded(),
        );

        match response {
            SearchPlaneQueryIpcResponse::HybridSeed(hybrid_seed) => {
                let response_json = serde_json::to_value(&hybrid_seed)?;
                if response_json
                    .get("manifest_digest")
                    .and_then(serde_json::Value::as_str)
                    != Some("manifest-digest-9")
                {
                    return Err(format!(
                        "hybrid seed response must carry the sealed semantic manifest digest, observed={response_json}"
                    )
                    .into());
                }
                let seed_candidates = hybrid_seed.seed_candidates;
                if !seed_candidates
                    .iter()
                    .any(|candidate| candidate.entity_id == "owner:SymbolCard")
                {
                    return Err(format!(
                        "dense-only semantic entity must enter v2 seed set, observed={seed_candidates:?}"
                    )
                    .into());
                }
                let cluster_seed = seed_candidates
                    .iter()
                    .find(|candidate| {
                        candidate.corpus_kind == Some(SemanticCorpusKindV1::ClusterCard)
                    })
                    .ok_or_else(|| {
                        format!(
                            "requested ClusterCard lane must reach the hybrid seed response: {seed_candidates:?}"
                        )
                    })?;
                if cluster_seed.authority_digest.as_deref() != Some("authority:ClusterCard") {
                    return Err(format!(
                        "ClusterCard record authority must survive semantic search and seed assembly: {cluster_seed:?}"
                    )
                    .into());
                }
                if !seed_candidates.iter().all(|candidate| {
                    candidate.degraded_reasons.iter().any(|reason| {
                        reason == "requested_semantic_corpus_unavailable:RepositorySummary"
                    })
                }) {
                    return Err(format!(
                        "missing requested corpus must remain explicit on every returned seed: {seed_candidates:?}"
                    )
                    .into());
                }
            }
            other @ (SearchPlaneQueryIpcResponse::Hybrid(_)
            | SearchPlaneQueryIpcResponse::Text(_)
            | SearchPlaneQueryIpcResponse::Symbol(_)
            | SearchPlaneQueryIpcResponse::Semantic(_)
            | SearchPlaneQueryIpcResponse::History(_)
            | SearchPlaneQueryIpcResponse::Structural(_)
            | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
            | SearchPlaneQueryIpcResponse::Explain(_)
            | SearchPlaneQueryIpcResponse::Error(_)
            | quanta_index_contract::SearchPlaneQueryIpcResponse::ClusterMembershipRead(
                _,
            )
            | quanta_index_contract::SearchPlaneQueryIpcResponse::RuntimeMetadata(_)) => {
                return Err(format!("expected HybridSeed response, got {other:?}").into());
            }
        }

        let (
            scoped_vectors,
            search_hit_vectors,
            search_vectors,
            corpus_searches,
            scoped_constraints,
            corpus_constraints,
        ) = {
            let guard = semantic_state
                .lock()
                .map_err(|err| format!("semantic state poisoned: {err}"))?;
            (
                guard.scoped_vectors.clone(),
                guard.search_hit_vectors.clone(),
                guard.search_vectors.clone(),
                guard.corpus_searches.clone(),
                guard.scoped_constraints.clone(),
                guard.corpus_constraints.clone(),
            )
        };
        let expected = default_query_embedder().embed_query("scope alpha")?;
        // QI-BB-019: the seed list is built from the dense lanes alone; no
        // second, lexical-scoped dense search runs behind it.
        if !scoped_vectors.is_empty() {
            return Err(format!(
                "hybrid seed must not run a lexical-scoped dense search: {scoped_vectors:?}"
            )
            .into());
        }
        // Exactly one dense search per requested corpus lane, all over the
        // one query vector.
        if search_hit_vectors.as_slice() != [expected.clone(), expected.clone(), expected] {
            return Err(format!("unexpected corpus hit vectors: {search_hit_vectors:?}").into());
        }
        if !search_vectors.is_empty() {
            return Err(
                format!("unexpected global lexical-shaped vectors: {search_vectors:?}").into(),
            );
        }
        if corpus_searches.as_slice()
            != [
                (SemanticCorpusKindV1::ClusterCard, 13),
                (SemanticCorpusKindV1::RepositorySummary, 11),
                (SemanticCorpusKindV1::SymbolCard, 7),
            ]
        {
            return Err(
                format!("unexpected corpus-prefiltered searches: {corpus_searches:?}").into(),
            );
        }
        if !scoped_constraints.is_empty()
            || corpus_constraints.as_slice()
                != [
                    constraints.clone(),
                    constraints.clone(),
                    constraints.clone(),
                ]
        {
            return Err(format!(
                "hybrid-seed constraints drifted: scoped={scoped_constraints:?} corpus={corpus_constraints:?}"
            )
            .into());
        }
        let searched_constraints = {
            let guard = lexical_state
                .lock()
                .map_err(|err| format!("lexical state poisoned: {err}"))?;
            guard.searched_constraints.clone()
        };
        if searched_constraints.as_slice() != [constraints] {
            return Err(format!(
                "hybrid-seed lexical leg lost exact-path constraints: {searched_constraints:?}"
            )
            .into());
        }
        Ok(())
    }

    #[test]
    fn hybrid_seed_keeps_cross_owner_ids_and_corpus_local_ranks_distinct() -> TestResult {
        fn hit(
            record_id: &str,
            owner_id: &str,
            corpus_kind: SemanticCorpusKindV1,
            score: f32,
        ) -> SemanticSearchHitV1 {
            SemanticSearchHitV1 {
                candidate: candidate(record_id, score),
                record_id: record_id.to_string(),
                owner_id: owner_id.to_string(),
                owner_kind: match corpus_kind {
                    SemanticCorpusKindV1::SymbolCard => quanta_index_contract::OwnerDocKind::Symbol,
                    SemanticCorpusKindV1::ModuleCard => quanta_index_contract::OwnerDocKind::Module,
                    SemanticCorpusKindV1::ClusterCard
                    | SemanticCorpusKindV1::RawCodeFallback
                    | SemanticCorpusKindV1::DocumentLeaf
                    | SemanticCorpusKindV1::DocumentSection
                    | SemanticCorpusKindV1::DocumentSummary
                    | SemanticCorpusKindV1::TestBehavior
                    | SemanticCorpusKindV1::RepositorySummary => {
                        quanta_index_contract::OwnerDocKind::Chunk
                    }
                },
                corpus_kind: Some(corpus_kind),
                authority_digest: format!("authority:{record_id}"),
            }
        }

        let semantic_lanes = vec![
            vec![
                hit(
                    "module-shared",
                    "shared",
                    SemanticCorpusKindV1::ModuleCard,
                    0.0001,
                ),
                hit(
                    "module-only",
                    "module-only",
                    SemanticCorpusKindV1::ModuleCard,
                    9_999.0,
                ),
            ],
            vec![
                hit(
                    "symbol-shared",
                    "shared",
                    SemanticCorpusKindV1::SymbolCard,
                    0.0002,
                ),
                hit(
                    "symbol-only",
                    "symbol-only",
                    SemanticCorpusKindV1::SymbolCard,
                    8_888.0,
                ),
            ],
        ];
        let seeds = build_hybrid_seed_candidates(&[], &semantic_lanes, &[], 3)?;
        let module_shared = seeds
            .iter()
            .find(|seed| {
                seed.entity_id == "shared"
                    && seed.owner_kind == quanta_index_contract::OwnerDocKind::Module
            })
            .ok_or_else(|| "expected Module/shared seed".to_string())?;
        let symbol_shared = seeds
            .iter()
            .find(|seed| {
                seed.entity_id == "shared"
                    && seed.owner_kind == quanta_index_contract::OwnerDocKind::Symbol
            })
            .ok_or_else(|| "expected Symbol/shared seed".to_string())?;

        for (seed, expected_corpus) in [
            (module_shared, SemanticCorpusKindV1::ModuleCard),
            (symbol_shared, SemanticCorpusKindV1::SymbolCard),
        ] {
            let contribution = seed.contributions.first().ok_or_else(|| {
                format!("cross-owner seed must retain one contribution: {seed:?}")
            })?;
            if seed.contributions.len() != 1
                || contribution.corpus_kind != Some(expected_corpus)
                || contribution.rank != 1
            {
                return Err(format!(
                    "cross-owner seed must retain one rank-1 corpus-local contribution: {seed:?}"
                )
                .into());
            }
        }
        Ok(())
    }

    #[test]
    fn hybrid_seed_preserves_authoritative_semantic_owner_kind() -> TestResult {
        let semantic_lanes = vec![vec![SemanticSearchHitV1 {
            candidate: candidate("test-behavior-record", 0.9),
            record_id: "test-behavior-record".to_string(),
            owner_id: "test:session_commit".to_string(),
            owner_kind: quanta_index_contract::OwnerDocKind::Test,
            corpus_kind: Some(SemanticCorpusKindV1::TestBehavior),
            authority_digest: "authority:test-behavior-record".to_string(),
        }]];

        let seeds = build_hybrid_seed_candidates(&[], &semantic_lanes, &[], 1)?;
        let seed = seeds
            .first()
            .ok_or_else(|| "expected one semantic seed".to_string())?;
        if seed.owner_kind != quanta_index_contract::OwnerDocKind::Test {
            return Err(format!(
                "semantic seed must preserve Test owner kind independently of corpus kind: {seed:?}"
            )
            .into());
        }
        Ok(())
    }

    #[test]
    fn semantic_dispatch_rejects_active_digest_mismatch_with_exact_code() -> TestResult {
        let dir = tempdir()?;
        let activation_catalog = Arc::new(ActivationCatalog::open(dir.keep())?);
        let active = corpus_generation(
            RepoId::new("repo-map-ipc"),
            RevisionId::new("rev-map-ipc"),
            ManifestGeneration::new(9),
            "activation-digest-9",
        )?;
        let prepared = PreparedSearchCorpusGenerationV1::new(active, None)?;
        let activation =
            activation_catalog.activate_prepared_search_corpus_generation_v1(&prepared)?;
        if activation.active.manifest_generation() != ManifestGeneration::new(9) {
            return Err("expected active composite generation 9".into());
        }
        let mut ledger = Ledger::default();
        let repo_id = RepoId::new("repo-map-ipc");
        let revision_id = RevisionId::new("rev-map-ipc");
        ledger.record_track_materialized(
            &repo_id,
            &revision_id,
            SearchPlaneTrackKind::Semantic,
            ManifestGeneration::new(9),
            Some("observed-digest-9"),
        );
        ledger.record_track_seal_with_digest(
            &repo_id,
            &revision_id,
            SearchPlaneTrackKind::Semantic,
            ManifestGeneration::new(9),
            "observed-digest-9",
        );
        let dispatcher = SearchPlaneDispatcher::new(
            Arc::new(RejectLexicalOpener),
            Arc::new(RejectSemanticOpener),
            Arc::new(StubRepoMapQueryPort),
            Arc::new(FailClosedStructuralProducer),
            Arc::new(RwLock::new(ledger)),
            activation_catalog,
        );

        let response = dispatcher.dispatch(
            SearchPlaneQueryIpcRequest::Semantic(SemanticQueryRequest {
                query_text: "focus alpha".to_string(),
                constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                generation: None,
                generation_selector: Some(GenerationSelector::Active {
                    repo_id: RepoId::new("repo-map-ipc"),
                    revision_id: RevisionId::new("rev-map-ipc"),
                }),
                lexical_scope: None,
                top_k: 3,
            }),
            &RequestBudgetV1::unbounded(),
        );

        let (code, message) =
            ipc_error_from(response).map_err(Box::<dyn std::error::Error>::from)?;
        if code != crate::readiness::ERR_SEMANTIC_MANIFEST_DIGEST_MISMATCH {
            return Err(format!("unexpected semantic mismatch code: {code}").into());
        }
        if !message.contains("expected=activation-digest-9")
            || !message.contains("observed=observed-digest-9")
        {
            return Err(format!("unexpected semantic mismatch message: {message}").into());
        }
        Ok(())
    }

    #[test]
    fn semantic_dispatch_rejects_unsealed_pinned_generation_with_exact_code() -> TestResult {
        let mut ledger = Ledger::default();
        let repo_id = RepoId::new("repo-map-ipc");
        let revision_id = RevisionId::new("rev-map-ipc");
        ledger.record_track_materialized(
            &repo_id,
            &revision_id,
            SearchPlaneTrackKind::Semantic,
            ManifestGeneration::new(9),
            Some("manifest-digest-9"),
        );
        let dispatcher = SearchPlaneDispatcher::new(
            Arc::new(RejectLexicalOpener),
            Arc::new(RejectSemanticOpener),
            Arc::new(StubRepoMapQueryPort),
            Arc::new(FailClosedStructuralProducer),
            Arc::new(RwLock::new(ledger)),
            test_activation_catalog()?,
        );

        let response = dispatcher.dispatch(
            SearchPlaneQueryIpcRequest::Semantic(SemanticQueryRequest {
                query_text: "focus alpha".to_string(),
                constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                generation: Some(GenerationPin::new(
                    RepoId::new("repo-map-ipc"),
                    RevisionId::new("rev-map-ipc"),
                    ManifestGeneration::new(9),
                )),
                generation_selector: None,
                lexical_scope: None,
                top_k: 3,
            }),
            &RequestBudgetV1::unbounded(),
        );

        let (code, _message) =
            ipc_error_from(response).map_err(Box::<dyn std::error::Error>::from)?;
        if code != crate::readiness::ERR_SEMANTIC_GENERATION_NOT_SEALED {
            return Err(format!("unexpected semantic unsealed code: {code}").into());
        }
        Ok(())
    }

    #[test]
    fn hybrid_dispatch_rejects_unsealed_semantic_generation_with_exact_code() -> TestResult {
        let mut ledger = Ledger::default();
        let repo_id = RepoId::new("repo-map-ipc");
        let revision_id = RevisionId::new("rev-map-ipc");
        ledger.record_track_materialized(
            &repo_id,
            &revision_id,
            SearchPlaneTrackKind::Lexical,
            ManifestGeneration::new(9),
            None,
        );
        ledger.record_track_seal(
            &repo_id,
            &revision_id,
            SearchPlaneTrackKind::Lexical,
            ManifestGeneration::new(9),
        );
        ledger.record_track_materialized(
            &repo_id,
            &revision_id,
            SearchPlaneTrackKind::Semantic,
            ManifestGeneration::new(9),
            Some("manifest-digest-9"),
        );
        let dispatcher = SearchPlaneDispatcher::new(
            Arc::new(RejectLexicalOpener),
            Arc::new(RejectSemanticOpener),
            Arc::new(StubRepoMapQueryPort),
            Arc::new(FailClosedStructuralProducer),
            Arc::new(RwLock::new(ledger)),
            test_activation_catalog()?,
        );

        let response = dispatcher.dispatch(
            SearchPlaneQueryIpcRequest::Hybrid(HybridQueryRequest {
                text_query: TextQueryRequest {
                    syntax: TextQuerySyntax::Native,
                    query_text: "scope".to_string(),
                    constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                    generation: Some(ready_pin()),
                    generation_selector: None,
                    top_k: 2,
                },
                semantic_query_text: "scope alpha".to_string(),
                generation: Some(ready_pin()),
                generation_selector: None,
                top_k: 2,
            }),
            &RequestBudgetV1::unbounded(),
        );

        let (code, _message) =
            ipc_error_from(response).map_err(Box::<dyn std::error::Error>::from)?;
        if code != crate::readiness::ERR_SEMANTIC_GENERATION_NOT_SEALED {
            return Err(format!("unexpected hybrid unsealed code: {code}").into());
        }
        Ok(())
    }

    // ------------------------------------------------------------------
    // LXE-02 / LXE-09 wiring tests.
    //
    // These exercise the new planner short-circuit and the structural
    // domain-port routing path. They are additive — existing dispatcher
    // tests remain unchanged.
    // ------------------------------------------------------------------

    use std::sync::atomic::{AtomicUsize, Ordering};

    use quanta_index_contract::SearchPlaneTrackKind;
    use quanta_index_core::domains::structural::{
        StructuralError, StructuralProducerPort, StructuralQueryRequest, StructuralReadiness,
    };
    use quanta_index_core::{StructuralMatchBinding, StructuralMatchCandidate};

    /// Test producer that records how many times `readiness` was consulted
    /// and lets a test choose which readiness value is returned.
    struct RecordingStructuralProducer {
        readiness: StructuralReadiness,
        results: Vec<StructuralMatchCandidate>,
        execute_error: Option<StructuralError>,
        readiness_calls: AtomicUsize,
        execute_calls: AtomicUsize,
    }

    impl RecordingStructuralProducer {
        fn new(readiness: StructuralReadiness) -> Self {
            Self {
                readiness,
                results: Vec::new(),
                execute_error: None,
                readiness_calls: AtomicUsize::new(0),
                execute_calls: AtomicUsize::new(0),
            }
        }

        fn ready_with(results: Vec<StructuralMatchCandidate>) -> Self {
            Self {
                readiness: StructuralReadiness::Ready,
                results,
                execute_error: None,
                readiness_calls: AtomicUsize::new(0),
                execute_calls: AtomicUsize::new(0),
            }
        }

        fn ready_with_error(execute_error: StructuralError) -> Self {
            Self {
                readiness: StructuralReadiness::Ready,
                results: Vec::new(),
                execute_error: Some(execute_error),
                readiness_calls: AtomicUsize::new(0),
                execute_calls: AtomicUsize::new(0),
            }
        }
    }

    impl StructuralProducerPort for RecordingStructuralProducer {
        fn readiness(&self, _request: &StructuralQueryRequest) -> StructuralReadiness {
            let _prev: usize = self.readiness_calls.fetch_add(1, Ordering::SeqCst);
            self.readiness.clone()
        }

        fn execute(
            &self,
            _request: &StructuralQueryRequest,
        ) -> Result<Vec<StructuralMatchCandidate>, StructuralError> {
            let _prev: usize = self.execute_calls.fetch_add(1, Ordering::SeqCst);
            if let Some(err) = self.execute_error.as_ref() {
                return Err(match err {
                    StructuralError::ParseTreeProducerUnavailable => {
                        StructuralError::ParseTreeProducerUnavailable
                    }
                    StructuralError::GenerationNotReady => StructuralError::GenerationNotReady,
                    StructuralError::ShardUnavailable => StructuralError::ShardUnavailable,
                    StructuralError::LangNotSupported(lang) => {
                        StructuralError::LangNotSupported(lang.clone())
                    }
                    StructuralError::HoleKindUnsupported(kind) => {
                        StructuralError::HoleKindUnsupported(kind.clone())
                    }
                    StructuralError::InvalidRequest(message) => {
                        StructuralError::InvalidRequest(message.clone())
                    }
                    StructuralError::ProducerExecution(message) => {
                        StructuralError::ProducerExecution(message.clone())
                    }
                });
            }
            Ok(self.results.clone())
        }
    }

    struct PatternRoutingStructuralProducer {
        readiness_calls: AtomicUsize,
        execute_calls: AtomicUsize,
        candidate_scopes: Mutex<Vec<Option<Vec<String>>>>,
    }

    impl PatternRoutingStructuralProducer {
        fn new() -> Self {
            Self {
                readiness_calls: AtomicUsize::new(0),
                execute_calls: AtomicUsize::new(0),
                candidate_scopes: Mutex::new(Vec::new()),
            }
        }

        fn recorded_scopes(&self) -> Result<Vec<Option<Vec<String>>>, Box<dyn std::error::Error>> {
            let guard = self
                .candidate_scopes
                .lock()
                .map_err(|err| format!("candidate scope state poisoned: {err}"))?;
            Ok(guard.clone())
        }
    }

    impl StructuralProducerPort for PatternRoutingStructuralProducer {
        fn readiness(&self, _request: &StructuralQueryRequest) -> StructuralReadiness {
            let _prev: usize = self.readiness_calls.fetch_add(1, Ordering::SeqCst);
            StructuralReadiness::Ready
        }

        fn execute(
            &self,
            request: &StructuralQueryRequest,
        ) -> Result<Vec<StructuralMatchCandidate>, StructuralError> {
            let _prev: usize = self.execute_calls.fetch_add(1, Ordering::SeqCst);
            {
                let mut scopes = self.candidate_scopes.lock().map_err(|err| {
                    StructuralError::ProducerExecution(format!(
                        "candidate scope state poisoned: {err}"
                    ))
                })?;
                scopes.push(request.candidate_scope.clone());
            }
            match structural_pattern_key(&request.pattern) {
                Some("alpha") => Ok(vec![
                    structural_match_candidate_with_binding("chunk-a", 10, 15, "x", 10, 15),
                    structural_match_candidate_with_binding("chunk-shared", 20, 25, "x", 20, 25),
                    structural_match_candidate_with_binding("chunk-shared", 5, 10, "x", 5, 10),
                ]),
                Some("beta") => Ok(vec![structural_match_candidate_with_binding(
                    "chunk-shared",
                    30,
                    35,
                    "y",
                    30,
                    35,
                )]),
                Some("gamma") => Ok(vec![structural_match_candidate_with_binding(
                    "chunk-a", 40, 45, "z", 40, 45,
                )]),
                Some(other) => Err(StructuralError::InvalidRequest(format!(
                    "unexpected structural pattern key `{other}` in test producer"
                ))),
                None => Err(StructuralError::InvalidRequest(
                    "missing structural pattern key in test producer".to_string(),
                )),
            }
        }
    }

    fn structural_pattern_key(pattern: &quanta_index_contract::LqStructuralBlock) -> Option<&str> {
        match pattern.nodes.first() {
            Some(quanta_index_contract::LqStructuralNode::Literal(text)) => Some(text.trim()),
            _ => None,
        }
    }

    fn structural_dispatcher_with_producer<P>(
        producer: Arc<P>,
    ) -> Result<SearchPlaneDispatcher, Box<dyn std::error::Error>>
    where
        P: StructuralProducerPort + Send + Sync + 'static,
    {
        structural_dispatcher_with_producer_and_ledger(producer, ready_ledger())
    }

    fn structural_dispatcher_with_producer_and_ledger<P>(
        producer: Arc<P>,
        ledger: Arc<RwLock<Ledger>>,
    ) -> Result<SearchPlaneDispatcher, Box<dyn std::error::Error>>
    where
        P: StructuralProducerPort + Send + Sync + 'static,
    {
        Ok(SearchPlaneDispatcher::new(
            Arc::new(RejectLexicalOpener),
            Arc::new(RejectSemanticOpener),
            Arc::new(StubRepoMapQueryPort),
            producer,
            ledger,
            test_activation_catalog()?,
        ))
    }

    fn structural_dispatcher_mixed<P>(
        producer: Arc<P>,
        lex_opener: Arc<dyn LexicalIndexOpenPort + Send + Sync>,
        ledger: Arc<RwLock<Ledger>>,
    ) -> Result<SearchPlaneDispatcher, Box<dyn std::error::Error>>
    where
        P: StructuralProducerPort + Send + Sync + 'static,
    {
        Ok(SearchPlaneDispatcher::new(
            lex_opener,
            Arc::new(RejectSemanticOpener),
            Arc::new(StubRepoMapQueryPort),
            producer,
            ledger,
            test_activation_catalog()?,
        ))
    }

    fn ready_ledger_with_structural_boolean_chunks() -> Arc<RwLock<Ledger>> {
        let mut ledger = Ledger::default();
        let repo_id = RepoId::new("repo-map-ipc");
        let revision_id = RevisionId::new("rev-map-ipc");
        let generation = ManifestGeneration::new(9);
        ledger.lexical_seal(generation);
        ledger.semantic_seal_with_digest(generation, "manifest-digest-9");
        ledger.record_track_materialized(
            &repo_id,
            &revision_id,
            SearchPlaneTrackKind::Lexical,
            generation,
            None,
        );
        ledger.record_track_seal(
            &repo_id,
            &revision_id,
            SearchPlaneTrackKind::Lexical,
            generation,
        );
        ledger.record_track_materialized(
            &repo_id,
            &revision_id,
            SearchPlaneTrackKind::Semantic,
            generation,
            Some("manifest-digest-9"),
        );
        ledger.record_track_seal_with_digest(
            &repo_id,
            &revision_id,
            SearchPlaneTrackKind::Semantic,
            generation,
            "manifest-digest-9",
        );
        for (chunk_id, path, text) in [
            ("chunk-a", "src/a.rs", "alpha text"),
            ("chunk-shared", "src/shared.rs", "alpha beta text"),
            ("chunk-beta", "src/b.rs", "beta text"),
        ] {
            install_structural_test_chunk(&mut ledger, chunk_id, path, text)
                .expect("structural test chunk install");
        }
        Arc::new(RwLock::new(ledger))
    }

    fn ready_runtime_metadata_ledger(
        producer_head_applied_at_ms: u64,
        generation_materialized_at_ms: u64,
    ) -> Arc<RwLock<Ledger>> {
        let ledger = ready_ledger();
        {
            let mut guard = ledger
                .write()
                .expect("runtime metadata test ledger poisoned");
            for (chunk_id, path, text) in [
                ("chunk-dirty", "src/dirty.rs", "todo dirty"),
                ("chunk-clean", "src/clean.rs", "todo clean"),
                ("chunk-changed", "src/changed.rs", "catalog changed"),
                ("chunk-stale", "src/stale.rs", "catalog stale"),
                ("chunk-snapshot", "src/snapshot.rs", "catalog snapshot"),
                ("chunk-owner", "src/owner.rs", "catalog owner"),
            ] {
                install_structural_test_chunk(&mut guard, chunk_id, path, text)
                    .expect("runtime metadata test chunk install");
            }
            guard.apply_runtime_batch(&DirtyIngestBatch {
                repo_id: RepoId::new("repo-map-ipc"),
                revision_id: RevisionId::new("rev-map-ipc"),
                generation: ManifestGeneration::new(9),
                overlay_epoch_ms: 100,
                batch_digest: "dirty:test".to_string(),
                entries: vec![DirtyMutation::Upsert(DirtyRecord {
                    wire_version: 1,
                    doc_id: ChunkId::new("chunk-dirty"),
                    applied_at_ms: 100,
                    payload_hash: [0x5a; 32],
                })],
            });
            guard
                .apply_runtime_catalog_batch(&RuntimeCatalogIngestBatch {
                    repo_id: RepoId::new("repo-map-ipc"),
                    revision_id: RevisionId::new("rev-map-ipc"),
                    generation: ManifestGeneration::new(9),
                    overlay_epoch_ms: 20,
                    batch_digest: "catalog:test".to_string(),
                    producer_head_applied_at_ms,
                    generation_materialized_at_ms,
                    changed_entries: vec![RuntimeChangedRecord {
                        doc_id: ChunkId::new("chunk-changed"),
                        applied_at_ms: 25,
                        payload_hash: [0xaa; 32],
                    }],
                    facet_entries: vec![RuntimeDocFacetRecord {
                        doc_id: ChunkId::new("chunk-owner"),
                        owner: Some("team-a".to_string()),
                        service: Some("search".to_string()),
                        layer: Some("index".to_string()),
                        surface: Some("lexical".to_string()),
                    }],
                    snapshot_entries: vec![RuntimeSnapshotRecord {
                        name: "active".to_string(),
                        doc_ids: vec![ChunkId::new("chunk-snapshot")],
                    }],
                    affected_entries: vec![RuntimeEdgeAuthorityRecord {
                        key: "rebuild=lexical".to_string(),
                        doc_ids: vec![ChunkId::new("chunk-changed")],
                    }],
                    invalidated_by_entries: vec![RuntimeEdgeAuthorityRecord {
                        key: "rebuild=lexical".to_string(),
                        doc_ids: vec![ChunkId::new("chunk-changed")],
                    }],
                })
                .expect("runtime metadata test catalog install");
        }
        ledger
    }

    fn install_structural_test_chunk(
        ledger: &mut Ledger,
        chunk_id: &str,
        path: &str,
        text: &str,
    ) -> TestResult {
        let mut record = structural_test_chunk_record(path, text);
        record.chunk_id = ChunkId::new(chunk_id);
        let op = LexicalChannelOp::UpsertChunk(UpsertChunk {
            repo_id: RepoId::new("repo-map-ipc"),
            revision_id: RevisionId::new("rev-map-ipc"),
            generation: ManifestGeneration::new(9),
            chunk_id: ChunkId::new(chunk_id),
            payload: encode_cbor(&record)?,
        });
        ledger.apply_lexical_authority_op(&op)?;
        Ok(())
    }

    fn structural_test_chunk_record(path: &str, text: &str) -> ChunkRecord {
        #[expect(
            clippy::manual_unwrap_or,
            clippy::option_if_let_else,
            reason = "Result::unwrap_or is disallowed by clippy.toml; saturate the test text length to u32::MAX"
        )]
        let end_byte = match u32::try_from(text.len()) {
            Ok(len) => len,
            Err(_) => u32::MAX,
        };
        ChunkRecord {
            chunk_id: ChunkId::new("chunk-1"),
            repo_relative_path: RepoRelativePath::new(path),
            language: LanguageCode::from_code_str("rust").expect("rust language code"),
            start_byte: 0,
            end_byte,
            start_line: 1,
            end_line: 1,
            text: text.to_string().into_boxed_str(),
            structural: None,
            parent_chunk_id: None,
            source_repo_id: None,
        }
    }

    fn structural_state_for_test_chunks(
        chunks: &[(&str, &str, &str)],
    ) -> Result<crate::readiness::StructuralAuthorityState, Box<dyn std::error::Error>> {
        let ledger = ready_ledger_with_structural_boolean_chunks();
        {
            let mut guard = ledger.write().expect("structural test ledger poisoned");
            for (chunk_id, path, text) in chunks {
                install_structural_test_chunk(&mut guard, chunk_id, path, text)?;
            }
        }
        let guard = ledger.read().expect("structural test ledger poisoned");
        guard
            .structural_state(
                &RepoId::new("repo-map-ipc"),
                &RevisionId::new("rev-map-ipc"),
                ManifestGeneration::new(9),
            )
            .cloned()
            .ok_or_else(|| "structural state missing for test chunks".into())
    }

    fn recording_lexical_candidate(candidate_id: &str) -> LexicalCandidate {
        LexicalCandidate {
            candidate_id: candidate_id.to_string(),
            repo_id: RepoId::new("repo-map-ipc"),
            revision_id: RevisionId::new("rev-map-ipc"),
            manifest_generation: ManifestGeneration::new(9),
            repo_relative_path: RepoRelativePath::new("src/a.rs"),
            start_line: 1,
            end_line: 1,
            score: 1.0,
            snippet: candidate_id.to_string(),
            snippet_hit_offset: None,
            highlights: Vec::new(),
        }
    }

    fn dispatcher_with_obs(
        lex_opener: Arc<dyn LexicalIndexOpenPort + Send + Sync>,
        sem_opener: Arc<dyn SemanticIndexOpenPort + Send + Sync>,
        obs_sink: Arc<dyn QueryObsSink + Send + Sync>,
    ) -> Result<SearchPlaneDispatcher, Box<dyn std::error::Error>> {
        Ok(SearchPlaneDispatcher::new_with_obs(
            lex_opener,
            sem_opener,
            SnapshotRegistries::new(SnapshotRegistryPolicy::DEFAULT),
            Arc::new(StubRepoMapQueryPort),
            Arc::new(FailClosedStructuralProducer),
            ready_ledger(),
            test_activation_catalog()?,
            default_query_embedder(),
            obs_sink,
        ))
    }

    #[test]
    #[expect(
        clippy::panic_in_result_fn,
        reason = "`assert!`/`assert_eq!` invariant checks in a Result-returning test; a failed assertion is the intended test failure"
    )]
    fn symbol_hits_project_into_all_overlapping_chunks_deterministically()
    -> Result<(), Box<dyn std::error::Error>> {
        let structural_state = structural_state_for_test_chunks(&[
            (
                "chunk-symbol-left",
                "src/symbol.rs",
                "fn ParityTypeSymbol() {}",
            ),
            (
                "chunk-symbol-right",
                "src/symbol.rs",
                "fn ParityTypeSymbol() {}",
            ),
        ])?;
        let buckets = super::symbol_hits_to_structural_buckets(
            vec![SymbolCandidate {
                candidate_id: "symbol-hit-1".to_string(),
                repo_id: RepoId::new("repo-map-ipc"),
                revision_id: RevisionId::new("rev-map-ipc"),
                manifest_generation: ManifestGeneration::new(9),
                repo_relative_path: RepoRelativePath::new("src/symbol.rs"),
                start_line: 1,
                end_line: 1,
                score: 1.0,
                snippet: "ParityTypeSymbol".to_string(),
                symbol_kind: SymbolKindCode::from_code_str("function")
                    .expect("function symbol kind code"),
                symbol_kind_family: Some(SymbolKindFamily::Callable),
            }],
            &structural_state,
        );
        let projected_ids = buckets.keys().cloned().collect::<Vec<_>>();
        assert_eq!(
            projected_ids,
            vec![
                "chunk-symbol-left".to_string(),
                "chunk-symbol-right".to_string(),
            ]
        );
        for bucket in buckets.values() {
            assert_eq!(
                bucket.len(),
                1,
                "each overlapping chunk must receive exactly one projected structural bucket"
            );
        }
        Ok(())
    }

    fn ready_pin() -> quanta_index_contract::GenerationPin {
        make_pin(
            RepoId::new("repo-map-ipc"),
            RevisionId::new("rev-map-ipc"),
            ManifestGeneration::new(9),
        )
    }

    fn ipc_error_from(response: SearchPlaneQueryIpcResponse) -> Result<(String, String), String> {
        match response {
            SearchPlaneQueryIpcResponse::Error(err) => Ok((err.code, err.message)),
            other @ (SearchPlaneQueryIpcResponse::Text(_)
            | SearchPlaneQueryIpcResponse::Symbol(_)
            | SearchPlaneQueryIpcResponse::Semantic(_)
            | SearchPlaneQueryIpcResponse::Hybrid(_)
            | SearchPlaneQueryIpcResponse::HybridSeed(_)
            | SearchPlaneQueryIpcResponse::History(_)
            | SearchPlaneQueryIpcResponse::Structural(_)
            | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
            | SearchPlaneQueryIpcResponse::Explain(_)
            | SearchPlaneQueryIpcResponse::ClusterMembershipRead(_)
            | SearchPlaneQueryIpcResponse::RuntimeMetadata(_)) => {
                Err(format!("expected Error response, got {other:?}"))
            }
        }
    }

    fn assert_closed_obs_metrics(
        obs_sink: &Arc<BoundedQueryObsStore>,
        expected: &[&str],
    ) -> TestResult {
        let names = obs_sink
            .snapshot()
            .into_iter()
            .map(|sample| sample.name.into_string())
            .collect::<Vec<_>>();
        let expected = expected
            .iter()
            .map(|name| (*name).to_string())
            .collect::<Vec<_>>();
        if names != expected {
            return Err(format!("unexpected obs metric names: {names:?}").into());
        }
        let errors = obs_sink.errors();
        if !errors.is_empty() {
            return Err(format!("unexpected obs errors: {errors:?}").into());
        }
        let samples = obs_sink.snapshot();
        for sample in &samples {
            if sample.dimensions.ticket_id.as_ref() != "LXE-10"
                || sample.dimensions.wave_id.as_ref() != "8"
                || sample.dimensions.tenant_id.as_ref() != "local"
                || sample.dimensions.repo_id.as_ref() != "repo-map-ipc"
                || sample.dimensions.generation_id != 9
            {
                return Err(format!("unexpected obs dimensions: {:?}", sample.dimensions).into());
            }
            if sample.name.contains("needle")
                || sample.name.contains("alpha")
                || sample.name.contains("scope")
                || sample.name.contains("fix")
            {
                return Err(format!("metric name leaked query content: {}", sample.name).into());
            }
        }
        Ok(())
    }

    #[test]
    fn hybrid_dispatch_emits_closed_obs_metrics() -> TestResult {
        let obs_sink = Arc::new(BoundedQueryObsStore::default());
        let semantic_state = Arc::new(Mutex::new(RecordingSemanticState::default()));
        let dispatcher = dispatcher_with_obs(
            Arc::new(StubLexicalOpener {
                results: vec![candidate("lex-a", 1.0), candidate("lex-b", 0.5)],
            }),
            Arc::new(RecordingSemanticOpener {
                state: Arc::clone(&semantic_state),
            }),
            obs_sink.clone(),
        )?;

        let response = dispatcher.dispatch(
            SearchPlaneQueryIpcRequest::Hybrid(HybridQueryRequest {
                text_query: TextQueryRequest {
                    syntax: TextQuerySyntax::Native,
                    query_text: "scope".to_string(),
                    constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                    generation: Some(ready_pin()),
                    generation_selector: None,
                    top_k: 1,
                },
                semantic_query_text: "scope alpha".to_string(),
                generation: Some(ready_pin()),
                generation_selector: None,
                top_k: 1,
            }),
            &RequestBudgetV1::unbounded(),
        );
        match response {
            SearchPlaneQueryIpcResponse::Hybrid(_) | SearchPlaneQueryIpcResponse::HybridSeed(_) => {
            }
            other @ (SearchPlaneQueryIpcResponse::Text(_)
            | SearchPlaneQueryIpcResponse::Symbol(_)
            | SearchPlaneQueryIpcResponse::Semantic(_)
            | SearchPlaneQueryIpcResponse::History(_)
            | SearchPlaneQueryIpcResponse::ClusterMembershipRead(_)
            | SearchPlaneQueryIpcResponse::RuntimeMetadata(_)
            | SearchPlaneQueryIpcResponse::Structural(_)
            | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
            | SearchPlaneQueryIpcResponse::Explain(_)
            | SearchPlaneQueryIpcResponse::Error(_)) => {
                return Err(format!("expected Hybrid response, got {other:?}").into());
            }
        }

        let names = obs_sink
            .snapshot()
            .into_iter()
            .map(|sample| sample.name.into_string())
            .collect::<Vec<_>>();
        // The closed metric set for one cold hybrid dispatch. The two
        // snapshot samples are cold opens because the registry starts empty;
        // a warm dispatch would report `..._hit_total` in their place.
        let expected = vec![
            "lq_query_intake_total".to_string(),
            "lq_snapshot_lexical_cold_open_ms".to_string(),
            "lq_snapshot_semantic_cold_open_ms".to_string(),
            "lq_planner_total".to_string(),
            "lq_engine_fanout_count".to_string(),
            "lq_merge_result_count".to_string(),
            "lq_early_stop_total".to_string(),
        ];
        if names != expected {
            return Err(format!("unexpected obs metric names: {names:?}").into());
        }
        let errors = obs_sink.errors();
        if !errors.is_empty() {
            return Err(format!("unexpected obs errors: {errors:?}").into());
        }
        let samples = obs_sink.snapshot();
        for sample in &samples {
            if sample.dimensions.ticket_id.as_ref() != "LXE-10"
                || sample.dimensions.wave_id.as_ref() != "8"
                || sample.dimensions.tenant_id.as_ref() != "local"
                || sample.dimensions.repo_id.as_ref() != "repo-map-ipc"
                || sample.dimensions.generation_id != 9
            {
                return Err(format!("unexpected obs dimensions: {:?}", sample.dimensions).into());
            }
            if sample.name.contains("scope") || sample.name.contains("1.0 0.0") {
                return Err(format!("metric name leaked query content: {}", sample.name).into());
            }
        }
        Ok(())
    }

    #[test]
    #[expect(
        clippy::as_conversions,
        clippy::cast_precision_loss,
        reason = "test seeds distinct metric sample values from a small loop counter; usize->f64 is exact at these magnitudes"
    )]
    fn bounded_obs_store_evicts_oldest_samples_at_capacity() -> TestResult {
        let store = BoundedQueryObsStore::default();
        for i in 0..(MAX_OBS_SAMPLES + 8) {
            store.emit(MetricSample::new(
                format!("metric-{i}"),
                MetricKind::Counter,
                i as f64,
                Dimensions::new("LXE-10", "8", "local", "repo-map-ipc", 9),
            ));
        }
        let snapshot = store.snapshot();
        if snapshot.len() != MAX_OBS_SAMPLES {
            return Err(format!(
                "expected {} bounded samples, got {}",
                MAX_OBS_SAMPLES,
                snapshot.len()
            )
            .into());
        }
        if snapshot.first().map(|sample| sample.name.as_ref()) != Some("metric-8") {
            return Err(format!(
                "expected oldest retained sample to be metric-8, got {:?}",
                snapshot.first().map(|sample| sample.name.as_ref())
            )
            .into());
        }
        if snapshot.last().map(|sample| sample.name.as_ref()) != Some("metric-4103") {
            return Err(format!(
                "expected newest retained sample to be metric-4103, got {:?}",
                snapshot.last().map(|sample| sample.name.as_ref())
            )
            .into());
        }
        Ok(())
    }

    #[test]
    fn text_dispatch_parse_error_emits_closed_obs_metric() -> TestResult {
        let obs_sink = Arc::new(BoundedQueryObsStore::default());
        let dispatcher = dispatcher_with_obs(
            Arc::new(RejectLexicalOpener),
            Arc::new(RejectSemanticOpener),
            obs_sink.clone(),
        )?;

        let response = dispatcher.dispatch(
            SearchPlaneQueryIpcRequest::Text(TextQueryRequest {
                syntax: TextQuerySyntax::Native,
                query_text: "/(?<=needle_)x/".to_string(),
                constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                generation: Some(ready_pin()),
                generation_selector: None,
                top_k: 10,
            }),
            &RequestBudgetV1::unbounded(),
        );
        let (code, _message) =
            ipc_error_from(response).map_err(Box::<dyn std::error::Error>::from)?;
        if code != "PARSE_FAIL" {
            return Err(format!("expected PARSE_FAIL, got {code}").into());
        }
        let names = obs_sink
            .snapshot()
            .into_iter()
            .map(|sample| sample.name.into_string())
            .collect::<Vec<_>>();
        let expected = vec![
            "lq_query_intake_total".to_string(),
            "lq_typed_error_parse_total".to_string(),
        ];
        if names != expected {
            return Err(format!("unexpected parse-error obs metric names: {names:?}").into());
        }
        let errors = obs_sink.errors();
        if !errors.is_empty() {
            return Err(format!("unexpected parse-error obs errors: {errors:?}").into());
        }
        Ok(())
    }

    /// Every route observes its budget before it opens anything.
    ///
    /// An already-interrupted request is answered with the typed code naming
    /// the entry checkpoint, no opener is consulted (the reject openers would
    /// turn a consulted open into `NOT_IMPLEMENTED`), and the interruption
    /// lands in its own error counter rather than `other`.
    #[test]
    fn every_route_refuses_an_interrupted_budget_at_entry_without_opening() -> TestResult {
        let obs_sink = Arc::new(BoundedQueryObsStore::default());
        let dispatcher = dispatcher_with_obs(
            Arc::new(RejectLexicalOpener),
            Arc::new(RejectSemanticOpener),
            obs_sink.clone(),
        )?;
        let text = || TextQueryRequest {
            syntax: TextQuerySyntax::Native,
            query_text: "needle".to_string(),
            constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
            generation: Some(ready_pin()),
            generation_selector: None,
            top_k: 10,
        };
        let routes: Vec<(&str, SearchPlaneQueryIpcRequest)> = vec![
            ("lexical:entry", SearchPlaneQueryIpcRequest::Text(text())),
            (
                "symbol:entry",
                SearchPlaneQueryIpcRequest::Symbol(SymbolQueryRequest {
                    syntax: TextQuerySyntax::Native,
                    query_text: "needle".to_string(),
                    constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                    generation: Some(ready_pin()),
                    generation_selector: None,
                    top_k: 10,
                }),
            ),
            (
                "semantic:entry",
                SearchPlaneQueryIpcRequest::Semantic(semantic_focus_request()),
            ),
            (
                "hybrid:entry",
                SearchPlaneQueryIpcRequest::Hybrid(HybridQueryRequest {
                    text_query: text(),
                    semantic_query_text: "needle".to_string(),
                    generation: Some(ready_pin()),
                    generation_selector: None,
                    top_k: 10,
                }),
            ),
            ("history:entry", history_query_request("type:commit needle")),
            (
                "runtime-metadata:entry",
                runtime_query_request(TextQuerySyntax::Native, "dirty:only needle"),
            ),
            (
                "structural:entry",
                SearchPlaneQueryIpcRequest::Structural(
                    quanta_index_contract::StructuralQueryRequest { text_query: text() },
                ),
            ),
            (
                "repo-map:entry",
                SearchPlaneQueryIpcRequest::RepoMapQuery(repo_map_request()),
            ),
        ];
        let cancelled = RequestBudgetV1::unbounded();
        cancelled.cancel_handle().cancel();
        for (checkpoint, request) in routes {
            let (code, message) = ipc_error_from(dispatcher.dispatch(request, &cancelled))
                .map_err(Box::<dyn std::error::Error>::from)?;
            if code != REQUEST_CANCELLED_CODE {
                return Err(format!(
                    "{checkpoint}: expected REQUEST_CANCELLED, got {code}: {message}"
                )
                .into());
            }
            if !message.contains(&format!("checkpoint `{checkpoint}`")) {
                return Err(format!(
                    "{checkpoint}: interruption must name its checkpoint: {message}"
                )
                .into());
            }
        }
        let interrupted = obs_sink
            .snapshot()
            .into_iter()
            .filter(|sample| sample.name.as_ref() == "lq_typed_error_interrupted_total")
            .count();
        if interrupted != 8 {
            return Err(format!("expected 8 interrupted-error samples, got {interrupted}").into());
        }
        if obs_sink
            .snapshot()
            .iter()
            .any(|sample| sample.name.as_ref() == "lq_typed_error_other_total")
        {
            return Err("an interruption must not be counted as `other`".into());
        }
        Ok(())
    }

    #[test]
    fn repo_map_dispatch_emits_closed_obs_metrics() -> TestResult {
        let obs_sink = Arc::new(BoundedQueryObsStore::default());
        let dispatcher = SearchPlaneDispatcher::new_with_obs(
            Arc::new(RejectLexicalOpener),
            Arc::new(RejectSemanticOpener),
            SnapshotRegistries::new(SnapshotRegistryPolicy::DEFAULT),
            Arc::new(StubRepoMapQueryPort),
            Arc::new(FailClosedStructuralProducer),
            Arc::new(RwLock::new(Ledger::default())),
            test_activation_catalog()?,
            default_query_embedder(),
            obs_sink.clone(),
        );

        let response = dispatcher.dispatch(
            SearchPlaneQueryIpcRequest::RepoMapQuery(repo_map_request()),
            &RequestBudgetV1::unbounded(),
        );
        match response {
            SearchPlaneQueryIpcResponse::RepoMapQuery(_) => {}
            other @ (SearchPlaneQueryIpcResponse::Text(_)
            | SearchPlaneQueryIpcResponse::Symbol(_)
            | SearchPlaneQueryIpcResponse::Semantic(_)
            | SearchPlaneQueryIpcResponse::Hybrid(_)
            | SearchPlaneQueryIpcResponse::HybridSeed(_)
            | SearchPlaneQueryIpcResponse::History(_)
            | SearchPlaneQueryIpcResponse::ClusterMembershipRead(_)
            | SearchPlaneQueryIpcResponse::RuntimeMetadata(_)
            | SearchPlaneQueryIpcResponse::Structural(_)
            | SearchPlaneQueryIpcResponse::Explain(_)
            | SearchPlaneQueryIpcResponse::Error(_)) => {
                return Err(format!("expected RepoMapQuery response, got {other:?}").into());
            }
        }

        let names = obs_sink
            .snapshot()
            .into_iter()
            .map(|sample| sample.name.into_string())
            .collect::<Vec<_>>();
        let expected = vec![
            "lq_query_intake_total".to_string(),
            "lq_planner_total".to_string(),
            "lq_engine_fanout_count".to_string(),
            "lq_merge_result_count".to_string(),
        ];
        if names != expected {
            return Err(format!("unexpected repo-map obs metric names: {names:?}").into());
        }
        let errors = obs_sink.errors();
        if !errors.is_empty() {
            return Err(format!("unexpected repo-map obs errors: {errors:?}").into());
        }
        let samples = obs_sink.snapshot();
        for sample in &samples {
            if sample.dimensions.ticket_id.as_ref() != "LXE-10"
                || sample.dimensions.wave_id.as_ref() != "8"
                || sample.dimensions.tenant_id.as_ref() != "local"
                || sample.dimensions.repo_id.as_ref() != "repo-map-ipc"
                || sample.dimensions.generation_id != 9
            {
                return Err(format!(
                    "unexpected repo-map obs dimensions: {:?}",
                    sample.dimensions
                )
                .into());
            }
        }
        Ok(())
    }

    #[test]
    fn runtime_metadata_dispatch_not_ready_emits_closed_obs_metric() -> TestResult {
        let obs_sink = Arc::new(BoundedQueryObsStore::default());
        let dispatcher = dispatcher_with_obs(
            Arc::new(RejectLexicalOpener),
            Arc::new(RejectSemanticOpener),
            obs_sink.clone(),
        )?;

        let response = dispatcher.dispatch(
            SearchPlaneQueryIpcRequest::RuntimeMetadata(RuntimeMetadataQueryRequest {
                text_query: TextQueryRequest {
                    syntax: TextQuerySyntax::Native,
                    query_text: "changed:since=1970-01-01T00:00:00.010Z runtime".to_string(),
                    constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                    generation: Some(ready_pin()),
                    generation_selector: None,
                    top_k: 5,
                },
            }),
            &RequestBudgetV1::unbounded(),
        );
        let (code, _message) =
            ipc_error_from(response).map_err(Box::<dyn std::error::Error>::from)?;
        if code != "NOT_READY" {
            return Err(format!("expected NOT_READY, got {code}").into());
        }
        let names = obs_sink
            .snapshot()
            .into_iter()
            .map(|sample| sample.name.into_string())
            .collect::<Vec<_>>();
        let expected = vec![
            "lq_query_intake_total".to_string(),
            "lq_typed_error_not_ready_total".to_string(),
        ];
        if names != expected {
            return Err(format!("unexpected runtime-metadata obs metric names: {names:?}").into());
        }
        let errors = obs_sink.errors();
        if !errors.is_empty() {
            return Err(format!("unexpected runtime-metadata obs errors: {errors:?}").into());
        }
        Ok(())
    }

    #[test]
    fn runtime_metadata_dispatch_dirty_only_executes_like_dirty_yes() -> TestResult {
        let dispatcher =
            runtime_metadata_dispatcher_with_ledger(ready_runtime_metadata_ledger(100, 20))?;
        let response = dispatcher.dispatch(
            runtime_query_request(TextQuerySyntax::Native, "dirty:only todo"),
            &RequestBudgetV1::unbounded(),
        );
        let SearchPlaneQueryIpcResponse::RuntimeMetadata(response) = response else {
            return Err("expected RuntimeMetadata response".into());
        };
        let candidate_ids = response
            .results
            .into_iter()
            .map(|candidate| candidate.candidate_id)
            .collect::<Vec<_>>();
        if candidate_ids != ["chunk-dirty"] {
            return Err(format!("expected [\"chunk-dirty\"], got {candidate_ids:?}").into());
        }
        Ok(())
    }

    #[test]
    fn runtime_metadata_dispatch_rejects_predicate_leaf_typed_error() -> TestResult {
        let dispatcher = runtime_metadata_dispatcher_with_ledger(ready_ledger())?;
        let response = dispatcher.dispatch(
            runtime_query_request(
                TextQuerySyntax::Native,
                "changed:since=1970-01-01T00:00:00.010Z file.contains('catalog_changed_needle')",
            ),
            &RequestBudgetV1::unbounded(),
        );
        let (code, message) =
            ipc_error_from(response).map_err(Box::<dyn std::error::Error>::from)?;
        if code != ERR_NOT_IMPLEMENTED {
            return Err(format!("expected {ERR_NOT_IMPLEMENTED}, got {code}").into());
        }
        if !message.contains("runtime metadata: predicate leaves are not executable") {
            return Err(format!("unexpected predicate-leaf rejection message: {message}").into());
        }
        Ok(())
    }

    #[test]
    fn runtime_metadata_validate_rejects_content_predicate_leaf_upfront() -> TestResult {
        let query = manual_query(
            LqExpr::Leaf(LqLeaf::Keyword("catalog".to_string())),
            vec![
                LqFilter::Changed {
                    scope: "since=1970-01-01T00:00:00.010Z".to_string(),
                },
                LqFilter::Content {
                    leaf: LqLeaf::Predicate {
                        name: "file.contains".to_string(),
                        args: vec![LqPredicateArg::RawString("catalog".to_string())],
                    },
                },
            ],
        );
        match validate_runtime_metadata_query(&query) {
            Err(CoreError::NotImplemented(message))
                if message.contains("runtime metadata: predicate leaves are not executable") =>
            {
                Ok(())
            }
            other => Err(format!("expected predicate content reject, got {other:?}").into()),
        }
    }

    #[test]
    fn runtime_generation_is_stale_requires_producer_head_ahead() -> TestResult {
        let ledger = ready_runtime_metadata_ledger(20, 20);
        let guard = ledger
            .read()
            .map_err(|err| format!("runtime metadata test ledger poisoned: {err}"))?;
        let runtime = guard
            .runtime_state(
                &RepoId::new("repo-map-ipc"),
                &RevisionId::new("rev-map-ipc"),
                ManifestGeneration::new(9),
            )
            .ok_or("missing runtime metadata state")?;
        let is_stale = runtime_generation_is_stale(runtime, 30)?;
        drop(guard);
        if is_stale {
            return Err(
                "stale relation unexpectedly matched when producer head did not advance".into(),
            );
        }
        Ok(())
    }

    #[test]
    #[expect(
        clippy::significant_drop_tightening,
        reason = "guard borrows runtime and structural state used across the whole test"
    )]
    fn runtime_seed_ids_use_direct_catalog_sets() -> TestResult {
        let ledger = ready_runtime_metadata_ledger(100, 20);
        let guard = ledger
            .read()
            .map_err(|err| format!("runtime metadata test ledger poisoned: {err}"))?;
        let runtime = guard
            .runtime_state(
                &RepoId::new("repo-map-ipc"),
                &RevisionId::new("rev-map-ipc"),
                ManifestGeneration::new(9),
            )
            .ok_or("missing runtime metadata state")?;
        let structural = guard
            .structural_state(
                &RepoId::new("repo-map-ipc"),
                &RevisionId::new("rev-map-ipc"),
                ManifestGeneration::new(9),
            )
            .ok_or("missing structural state")?;

        let changed = runtime_seed_ids(
            &crate::lower_lexical_text_query(&TextQueryRequest {
                syntax: TextQuerySyntax::Native,
                query_text: "changed:since=1970-01-01T00:00:00.010Z catalog".to_string(),
                constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                generation: Some(ready_pin()),
                generation_selector: None,
                top_k: 5,
            })?,
            runtime,
            structural,
        )?;
        if changed != BTreeSet::from([ChunkId::new("chunk-changed")]) {
            return Err(format!("unexpected changed seed ids: {changed:?}").into());
        }

        let clean = runtime_seed_ids(
            &crate::lower_lexical_text_query(&TextQueryRequest {
                syntax: TextQuerySyntax::Native,
                query_text: "dirty:no todo".to_string(),
                constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                generation: Some(ready_pin()),
                generation_selector: None,
                top_k: 5,
            })?,
            runtime,
            structural,
        )?;
        if clean.contains(&ChunkId::new("chunk-dirty"))
            || !clean.contains(&ChunkId::new("chunk-clean"))
        {
            return Err(format!("unexpected clean-complement seed ids: {clean:?}").into());
        }

        let affected = runtime_seed_ids(
            &crate::lower_lexical_text_query(&TextQueryRequest {
                syntax: TextQuerySyntax::Native,
                query_text: "affected:rebuild=lexical catalog".to_string(),
                constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                generation: Some(ready_pin()),
                generation_selector: None,
                top_k: 5,
            })?,
            runtime,
            structural,
        )?;
        if affected != BTreeSet::from([ChunkId::new("chunk-changed")]) {
            return Err(format!("unexpected affected seed ids: {affected:?}").into());
        }
        Ok(())
    }

    #[test]
    fn history_dispatch_success_emits_closed_obs_metrics() -> TestResult {
        let obs_sink = Arc::new(BoundedQueryObsStore::default());
        let commit_payload = encode_cbor(&history_commit_record())?;
        let dispatcher = SearchPlaneDispatcher::new_with_obs(
            Arc::new(RejectLexicalOpener),
            Arc::new(RejectSemanticOpener),
            SnapshotRegistries::new(SnapshotRegistryPolicy::DEFAULT),
            Arc::new(StubRepoMapQueryPort),
            Arc::new(FailClosedStructuralProducer),
            ledger_with_history_ops(vec![LexicalChannelOp::UpsertCommit(UpsertCommit {
                repo_id: RepoId::new("repo-map-ipc"),
                revision_id: RevisionId::new("rev-map-ipc"),
                generation: ManifestGeneration::new(9),
                payload: commit_payload,
            })])?,
            test_activation_catalog()?,
            default_query_embedder(),
            obs_sink.clone(),
        );

        let response = dispatcher.dispatch(
            history_query_request("type:commit fix"),
            &RequestBudgetV1::unbounded(),
        );
        match response {
            SearchPlaneQueryIpcResponse::History(history) => {
                if history.generation != ready_pin()
                    || history.commits.len() != 1
                    || !history.diffs.is_empty()
                {
                    return Err(format!("unexpected history response: {history:?}").into());
                }
            }
            other @ (SearchPlaneQueryIpcResponse::Text(_)
            | SearchPlaneQueryIpcResponse::Symbol(_)
            | SearchPlaneQueryIpcResponse::Semantic(_)
            | SearchPlaneQueryIpcResponse::Hybrid(_)
            | SearchPlaneQueryIpcResponse::HybridSeed(_)
            | SearchPlaneQueryIpcResponse::ClusterMembershipRead(_)
            | SearchPlaneQueryIpcResponse::RuntimeMetadata(_)
            | SearchPlaneQueryIpcResponse::Structural(_)
            | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
            | SearchPlaneQueryIpcResponse::Explain(_)
            | SearchPlaneQueryIpcResponse::Error(_)) => {
                return Err(format!("expected History response, got {other:?}").into());
            }
        }

        assert_closed_obs_metrics(
            &obs_sink,
            &[
                "lq_query_intake_total",
                "lq_planner_total",
                "lq_engine_fanout_count",
                "lq_merge_result_count",
            ],
        )
    }

    #[test]
    fn history_dispatch_unavailable_emits_closed_obs_metric() -> TestResult {
        let obs_sink = Arc::new(BoundedQueryObsStore::default());
        let dispatcher = SearchPlaneDispatcher::new_with_obs(
            Arc::new(RejectLexicalOpener),
            Arc::new(RejectSemanticOpener),
            SnapshotRegistries::new(SnapshotRegistryPolicy::DEFAULT),
            Arc::new(StubRepoMapQueryPort),
            Arc::new(FailClosedStructuralProducer),
            ready_ledger(),
            test_activation_catalog()?,
            default_query_embedder(),
            obs_sink.clone(),
        );

        let response = dispatcher.dispatch(
            history_query_request("type:commit fix"),
            &RequestBudgetV1::unbounded(),
        );
        let (code, _message) =
            ipc_error_from(response).map_err(Box::<dyn std::error::Error>::from)?;
        if code != ERR_HISTORY_PRODUCER_UNAVAILABLE {
            return Err(format!("expected {ERR_HISTORY_PRODUCER_UNAVAILABLE}, got {code}").into());
        }

        assert_closed_obs_metrics(
            &obs_sink,
            &["lq_query_intake_total", "lq_typed_error_unavailable_total"],
        )
    }

    #[test]
    fn structural_dispatch_success_emits_closed_obs_metrics() -> TestResult {
        let obs_sink = Arc::new(BoundedQueryObsStore::default());
        let dispatcher = SearchPlaneDispatcher::new_with_obs(
            Arc::new(RejectLexicalOpener),
            Arc::new(RejectSemanticOpener),
            SnapshotRegistries::new(SnapshotRegistryPolicy::DEFAULT),
            Arc::new(StubRepoMapQueryPort),
            Arc::new(RecordingStructuralProducer::ready_with(vec![
                structural_match_candidate("chunk-tree"),
            ])),
            ready_ledger(),
            test_activation_catalog()?,
            default_query_embedder(),
            obs_sink.clone(),
        );

        let response = dispatcher.dispatch(
            SearchPlaneQueryIpcRequest::Structural(quanta_index_contract::StructuralQueryRequest {
                text_query: TextQueryRequest {
                    syntax: TextQuerySyntax::Native,
                    query_text: "match { :[x] }".to_string(),
                    constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                    generation: Some(ready_pin()),
                    generation_selector: None,
                    top_k: 4,
                },
            }),
            &RequestBudgetV1::unbounded(),
        );
        match response {
            SearchPlaneQueryIpcResponse::Structural(results) => {
                if results.generation != ready_pin() || results.results.len() != 1 {
                    return Err(format!("unexpected structural response: {results:?}").into());
                }
            }
            other @ (SearchPlaneQueryIpcResponse::Text(_)
            | SearchPlaneQueryIpcResponse::Symbol(_)
            | SearchPlaneQueryIpcResponse::Semantic(_)
            | SearchPlaneQueryIpcResponse::Hybrid(_)
            | SearchPlaneQueryIpcResponse::HybridSeed(_)
            | SearchPlaneQueryIpcResponse::History(_)
            | SearchPlaneQueryIpcResponse::ClusterMembershipRead(_)
            | SearchPlaneQueryIpcResponse::RuntimeMetadata(_)
            | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
            | SearchPlaneQueryIpcResponse::Explain(_)
            | SearchPlaneQueryIpcResponse::Error(_)) => {
                return Err(format!("expected Structural response, got {other:?}").into());
            }
        }

        assert_closed_obs_metrics(
            &obs_sink,
            &[
                "lq_query_intake_total",
                "lq_planner_total",
                "lq_engine_fanout_count",
                "lq_merge_result_count",
            ],
        )
    }

    #[test]
    fn classify_error_metric_name_uses_closed_taxonomy() {
        let cases = [
            (
                CoreError::Typed {
                    code: "PARSE_FAIL".to_string(),
                    message: "parse".to_string(),
                },
                "lq_typed_error_parse_total",
            ),
            (
                CoreError::Typed {
                    code: "BRIDGE_TRANSLATE_FAIL".to_string(),
                    message: "bridge".to_string(),
                },
                "lq_typed_error_parse_total",
            ),
            (
                CoreError::Typed {
                    code: "HISTORY_PRODUCER_UNAVAILABLE".to_string(),
                    message: "history".to_string(),
                },
                "lq_typed_error_unavailable_total",
            ),
            (
                CoreError::Typed {
                    code: "LEX_TRIGRAM_PLAN_LIMIT_EXCEEDED".to_string(),
                    message: "plan".to_string(),
                },
                "lq_typed_error_plan_limit_total",
            ),
            (
                CoreError::Typed {
                    code: REQUEST_DEADLINE_EXCEEDED_CODE.to_string(),
                    message: "deadline".to_string(),
                },
                "lq_typed_error_interrupted_total",
            ),
            (
                CoreError::Typed {
                    code: REQUEST_CANCELLED_CODE.to_string(),
                    message: "cancelled".to_string(),
                },
                "lq_typed_error_interrupted_total",
            ),
            (
                CoreError::Typed {
                    code: "QUERY_TIMEOUT".to_string(),
                    message: "timeout".to_string(),
                },
                "lq_typed_error_plan_limit_total",
            ),
            (
                CoreError::NotReady("replay".to_string()),
                "lq_typed_error_not_ready_total",
            ),
            (
                CoreError::Typed {
                    code: "STR_GENERATION_NOT_READY".to_string(),
                    message: "structural".to_string(),
                },
                "lq_typed_error_not_ready_total",
            ),
            (
                CoreError::Storage("disk".to_string()),
                "lq_typed_error_internal_total",
            ),
            (
                CoreError::InvalidContract("wire".to_string()),
                "lq_typed_error_invalid_request_total",
            ),
            (
                CoreError::Typed {
                    code: ERR_RUNTIME_DIRTY_ONLY_UNSUPPORTED.to_string(),
                    message: "dirty".to_string(),
                },
                "lq_typed_error_invalid_request_total",
            ),
            (
                CoreError::Typed {
                    code: "SEM_EXECUTION_ODDITY".to_string(),
                    message: "other".to_string(),
                },
                "lq_typed_error_other_total",
            ),
        ];

        for (err, expected) in cases {
            assert_eq!(classify_error_metric_name(&err), expected);
        }
    }

    fn structural_match_candidate(id: &str) -> StructuralMatchCandidate {
        structural_match_candidate_with_binding(id, 0, 10, "x", 0, 10)
    }

    fn structural_match_candidate_with_binding(
        id: &str,
        pattern_start_byte: u32,
        pattern_end_byte: u32,
        metavariable: &str,
        start_byte: u32,
        end_byte: u32,
    ) -> StructuralMatchCandidate {
        StructuralMatchCandidate {
            candidate_id: id.to_string(),
            pattern_start_byte,
            pattern_end_byte,
            bindings: vec![StructuralMatchBinding {
                metavariable: metavariable.to_string(),
                start_byte,
                end_byte,
                start_line: 1,
                end_line: 1,
            }],
        }
    }

    /// Repo-metadata-backed filters must reach the live searcher.
    ///
    /// The dispatcher no longer rejects `fork:` at routing time because the
    /// real searcher is the authority for whether repo metadata is present and
    /// can suppress the planner's conservative unavailable code.
    #[test]
    fn lexical_dispatch_returns_typed_when_filter_is_fork_only() -> TestResult {
        let state = Arc::new(Mutex::new(RecordingLexicalState::default()));
        let dispatcher = SearchPlaneDispatcher::new(
            Arc::new(RecordingLexicalOpener {
                state: Arc::clone(&state),
                results: Vec::new(),
            }),
            Arc::new(RejectSemanticOpener),
            Arc::new(StubRepoMapQueryPort),
            Arc::new(FailClosedStructuralProducer),
            ready_ledger(),
            test_activation_catalog()?,
        );

        let response = dispatcher.dispatch(
            SearchPlaneQueryIpcRequest::Text(TextQueryRequest {
                syntax: TextQuerySyntax::Native,
                query_text: "fork:only foo".to_string(),
                constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                generation: Some(ready_pin()),
                generation_selector: None,
                top_k: 5,
            }),
            &RequestBudgetV1::unbounded(),
        );

        match response {
            SearchPlaneQueryIpcResponse::Text(text) => {
                if !text.results.is_empty() {
                    return Err(format!(
                        "expected empty passthrough result set, got {:?}",
                        text.results
                    )
                    .into());
                }
            }
            other @ (SearchPlaneQueryIpcResponse::Symbol(_)
            | SearchPlaneQueryIpcResponse::Semantic(_)
            | SearchPlaneQueryIpcResponse::Hybrid(_)
            | SearchPlaneQueryIpcResponse::HybridSeed(_)
            | SearchPlaneQueryIpcResponse::History(_)
            | SearchPlaneQueryIpcResponse::ClusterMembershipRead(_)
            | SearchPlaneQueryIpcResponse::RuntimeMetadata(_)
            | SearchPlaneQueryIpcResponse::Structural(_)
            | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
            | SearchPlaneQueryIpcResponse::Explain(_)
            | SearchPlaneQueryIpcResponse::Error(_)) => {
                return Err(format!("expected Text response, got {other:?}").into());
            }
        }
        let search_top_ks = {
            let guard = state
                .lock()
                .map_err(|err| format!("lexical state poisoned: {err}"))?;
            guard.search_top_ks.clone()
        };
        if search_top_ks.as_slice() != [6] {
            return Err(format!(
                "expected metadata filter query to probe searcher with top_k=6, got {search_top_ks:?}"
            )
            .into());
        }
        Ok(())
    }

    #[test]
    fn history_dispatch_rejects_missing_type_with_invalid_request() -> TestResult {
        let dispatcher = history_dispatcher_with_ledger(ready_ledger())?;
        let response =
            dispatcher.dispatch(history_query_request("fix"), &RequestBudgetV1::unbounded());
        let (code, message) =
            ipc_error_from(response).map_err(Box::<dyn std::error::Error>::from)?;
        if code != ERR_INVALID {
            return Err(format!("expected {ERR_INVALID}, got {code}").into());
        }
        if !message.contains("explicit `type:commit` or `type:diff` is required") {
            return Err(format!("unexpected missing-type rejection message: {message}").into());
        }
        Ok(())
    }

    #[test]
    fn history_dispatch_rejects_commit_file_filter_with_invalid_request() -> TestResult {
        let dispatcher = history_dispatcher_with_ledger(ready_ledger())?;
        let response = dispatcher.dispatch(
            history_query_request("type:commit file:src/lib.rs fix"),
            &RequestBudgetV1::unbounded(),
        );
        let (code, message) =
            ipc_error_from(response).map_err(Box::<dyn std::error::Error>::from)?;
        if code != ERR_INVALID {
            return Err(format!("expected {ERR_INVALID}, got {code}").into());
        }
        if !message.contains("`file:` and `diff.*` filters require `type:diff`") {
            return Err(format!("unexpected commit-file rejection message: {message}").into());
        }
        Ok(())
    }

    #[test]
    fn history_dispatch_rejects_commit_diff_filter_with_invalid_request() -> TestResult {
        let dispatcher = history_dispatcher_with_ledger(ready_ledger())?;
        let response = dispatcher.dispatch(
            history_query_request("type:commit diff.added:history fix"),
            &RequestBudgetV1::unbounded(),
        );
        let (code, message) =
            ipc_error_from(response).map_err(Box::<dyn std::error::Error>::from)?;
        if code != ERR_INVALID {
            return Err(format!("expected {ERR_INVALID}, got {code}").into());
        }
        if !message.contains("`file:` and `diff.*` filters require `type:diff`") {
            return Err(format!("unexpected commit-diff rejection message: {message}").into());
        }
        Ok(())
    }

    #[test]
    fn history_dispatch_rejects_predicate_leaf_with_not_implemented() -> TestResult {
        let dispatcher = history_dispatcher_with_ledger(ready_ledger())?;
        let response = dispatcher.dispatch(
            history_query_request("type:commit file.contains('fix')"),
            &RequestBudgetV1::unbounded(),
        );
        let (code, message) =
            ipc_error_from(response).map_err(Box::<dyn std::error::Error>::from)?;
        if code != ERR_NOT_IMPLEMENTED {
            return Err(format!("expected {ERR_NOT_IMPLEMENTED}, got {code}").into());
        }
        if !message.contains("history: predicate leaves are not executable on this route") {
            return Err(
                format!("unexpected history predicate rejection message: {message}").into(),
            );
        }
        Ok(())
    }

    #[test]
    fn history_validate_rejects_content_regex_leaf_upfront() -> TestResult {
        let query = manual_query(
            LqExpr::Leaf(LqLeaf::Keyword("fix".to_string())),
            vec![
                LqFilter::Type {
                    kind: LqType::Commit,
                },
                LqFilter::Content {
                    leaf: LqLeaf::Regex("fix".to_string()),
                },
            ],
        );
        match validate_history_query(&query) {
            Err(CoreError::NotImplemented(message))
                if message.contains("history: regex leaves are not executable") =>
            {
                Ok(())
            }
            other => Err(format!("expected regex content reject, got {other:?}").into()),
        }
    }

    /// `rev:` remains fail-closed at the dispatcher boundary.
    ///
    /// Unlike repo-metadata-backed filters, `rev:` has no executable lexical
    /// rail today; `LexicalPolicy` rejects it before the searcher is opened.
    #[test]
    fn lexical_dispatch_returns_typed_when_filter_is_rev() -> TestResult {
        let dispatcher = SearchPlaneDispatcher::new(
            Arc::new(StubLexicalOpener {
                results: Vec::new(),
            }),
            Arc::new(RejectSemanticOpener),
            Arc::new(StubRepoMapQueryPort),
            Arc::new(FailClosedStructuralProducer),
            ready_ledger(),
            test_activation_catalog()?,
        );

        let response = dispatcher.dispatch(
            SearchPlaneQueryIpcRequest::Text(TextQueryRequest {
                syntax: TextQuerySyntax::Native,
                query_text: "rev:deadbeef foo".to_string(),
                constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                generation: Some(ready_pin()),
                generation_selector: None,
                top_k: 5,
            }),
            &RequestBudgetV1::unbounded(),
        );

        let (code, _message) =
            ipc_error_from(response).map_err(Box::<dyn std::error::Error>::from)?;
        if code != "NOT_IMPLEMENTED" {
            return Err(format!("expected NOT_IMPLEMENTED, got {code}").into());
        }
        Ok(())
    }

    #[test]
    fn lexical_dispatch_rebinds_rev_at_time_to_reachable_ancestor() -> TestResult {
        let state = Arc::new(Mutex::new(RecordingLexicalState::default()));
        let activation_catalog = activation_catalog_with_generations(&[
            corpus_generation(
                RepoId::new("repo-map-ipc"),
                RevisionId::new("1111111111111111111111111111111111111111"),
                ManifestGeneration::new(7),
                "ancestor-lex",
            )?,
            corpus_generation(
                RepoId::new("repo-map-ipc"),
                RevisionId::new("2222222222222222222222222222222222222222"),
                ManifestGeneration::new(9),
                "head-lex",
            )?,
        ])?;
        let dispatcher = SearchPlaneDispatcher::new(
            Arc::new(RecordingLexicalOpener {
                state: Arc::clone(&state),
                results: vec![candidate("ancestor-hit", 1.0)],
            }),
            Arc::new(RejectSemanticOpener),
            Arc::new(StubRepoMapQueryPort),
            Arc::new(FailClosedStructuralProducer),
            ledger_with_rev_at_time_history()?,
            activation_catalog,
        );

        let response = dispatcher.dispatch(
            SearchPlaneQueryIpcRequest::Text(TextQueryRequest {
                syntax: TextQuerySyntax::Sourcegraph,
                query_text: "rev:at.time(1970-01-01T00:00:00.150Z) foo".to_string(),
                constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                generation: Some(GenerationPin::new(
                    RepoId::new("repo-map-ipc"),
                    RevisionId::new("2222222222222222222222222222222222222222"),
                    ManifestGeneration::new(9),
                )),
                generation_selector: None,
                top_k: 5,
            }),
            &RequestBudgetV1::unbounded(),
        );

        match response {
            SearchPlaneQueryIpcResponse::Text(text) => {
                if text.generation
                    != GenerationPin::new(
                        RepoId::new("repo-map-ipc"),
                        RevisionId::new("1111111111111111111111111111111111111111"),
                        ManifestGeneration::new(7),
                    )
                {
                    return Err(
                        format!("unexpected rebound generation: {:?}", text.generation).into(),
                    );
                }
            }
            other @ (SearchPlaneQueryIpcResponse::Symbol(_)
            | SearchPlaneQueryIpcResponse::Semantic(_)
            | SearchPlaneQueryIpcResponse::Hybrid(_)
            | SearchPlaneQueryIpcResponse::HybridSeed(_)
            | SearchPlaneQueryIpcResponse::History(_)
            | SearchPlaneQueryIpcResponse::ClusterMembershipRead(_)
            | SearchPlaneQueryIpcResponse::RuntimeMetadata(_)
            | SearchPlaneQueryIpcResponse::Structural(_)
            | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
            | SearchPlaneQueryIpcResponse::Explain(_)
            | SearchPlaneQueryIpcResponse::Error(_)) => {
                return Err(format!("expected Text response, got {other:?}").into());
            }
        }

        let guard = state
            .lock()
            .map_err(|err| format!("lexical state poisoned: {err}"))?;
        if guard.opened_pins.as_slice()
            != [(
                RepoId::new("repo-map-ipc"),
                RevisionId::new("1111111111111111111111111111111111111111"),
                ManifestGeneration::new(7),
            )]
        {
            return Err(format!("unexpected opened pins: {:?}", guard.opened_pins).into());
        }
        let Some(last_query) = guard.searched_queries.last() else {
            return Err("expected search query after rev:at.time rebind".into());
        };
        if last_query
            .filters
            .iter()
            .any(|filter| matches!(filter, LqFilter::Rev { .. }))
        {
            return Err(format!(
                "rev filters must be consumed before lexical execution: {last_query:?}"
            )
            .into());
        }
        drop(guard);
        Ok(())
    }

    #[test]
    fn lexical_dispatch_rejects_rev_at_time_invalid_timeref() -> TestResult {
        let dispatcher = SearchPlaneDispatcher::new(
            Arc::new(StubLexicalOpener {
                results: Vec::new(),
            }),
            Arc::new(RejectSemanticOpener),
            Arc::new(StubRepoMapQueryPort),
            Arc::new(FailClosedStructuralProducer),
            ledger_with_rev_at_time_history()?,
            test_activation_catalog()?,
        );

        let response = dispatcher.dispatch(
            SearchPlaneQueryIpcRequest::Text(TextQueryRequest {
                syntax: TextQuerySyntax::Sourcegraph,
                query_text: "rev:at.time(definitely-not-a-timeref) foo".to_string(),
                constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                generation: Some(GenerationPin::new(
                    RepoId::new("repo-map-ipc"),
                    RevisionId::new("2222222222222222222222222222222222222222"),
                    ManifestGeneration::new(9),
                )),
                generation_selector: None,
                top_k: 5,
            }),
            &RequestBudgetV1::unbounded(),
        );

        let (code, _message) =
            ipc_error_from(response).map_err(Box::<dyn std::error::Error>::from)?;
        if code != ERR_HISTORY_INVALID_TIMEREF {
            return Err(format!("expected {ERR_HISTORY_INVALID_TIMEREF}, got {code}").into());
        }
        Ok(())
    }

    #[test]
    fn lexical_dispatch_rejects_rev_at_time_when_rebound_generation_is_unactivated() -> TestResult {
        let dispatcher = SearchPlaneDispatcher::new(
            Arc::new(StubLexicalOpener {
                results: Vec::new(),
            }),
            Arc::new(RejectSemanticOpener),
            Arc::new(StubRepoMapQueryPort),
            Arc::new(FailClosedStructuralProducer),
            ledger_with_rev_at_time_history()?,
            activation_catalog_with_generations(&[corpus_generation(
                RepoId::new("repo-map-ipc"),
                RevisionId::new("2222222222222222222222222222222222222222"),
                ManifestGeneration::new(9),
                "head-lex",
            )?])?,
        );

        let response = dispatcher.dispatch(
            SearchPlaneQueryIpcRequest::Text(TextQueryRequest {
                syntax: TextQuerySyntax::Sourcegraph,
                query_text: "rev:at.time(1970-01-01T00:00:00.150Z) foo".to_string(),
                constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                generation: Some(GenerationPin::new(
                    RepoId::new("repo-map-ipc"),
                    RevisionId::new("2222222222222222222222222222222222222222"),
                    ManifestGeneration::new(9),
                )),
                generation_selector: None,
                top_k: 5,
            }),
            &RequestBudgetV1::unbounded(),
        );

        let (code, message) =
            ipc_error_from(response).map_err(Box::<dyn std::error::Error>::from)?;
        if code != ERR_NOT_READY {
            return Err(format!("expected {ERR_NOT_READY}, got {code}").into());
        }
        if !message.contains("no active Lexical generation") {
            return Err(format!("unexpected rev:at.time not-ready message: {message}").into());
        }
        Ok(())
    }

    /// LXE-02: a query without unavailable filters reaches the searcher.
    /// The full hit shape is D1's territory; this test only proves no
    /// typed-unavailable error fires on the happy path.
    #[test]
    fn lexical_dispatch_passes_through_when_no_unavailable_filters() -> TestResult {
        let state = Arc::new(Mutex::new(RecordingLexicalState::default()));
        let dispatcher = SearchPlaneDispatcher::new(
            Arc::new(RecordingLexicalOpener {
                state: Arc::clone(&state),
                results: vec![candidate("hit", 1.0)],
            }),
            Arc::new(RejectSemanticOpener),
            Arc::new(StubRepoMapQueryPort),
            Arc::new(FailClosedStructuralProducer),
            ready_ledger(),
            test_activation_catalog()?,
        );

        // LXE-02 planner currently lowers only `LqExpr::Empty` and a single
        // `LqExpr::Leaf` shape; boolean composition (LXE-03) is unimplemented
        // and would surface `Unimplemented` from the planner. The happy-path
        // witness here is therefore a single keyword leaf — full multi-token
        // queries land with LXE-03.
        let response = dispatcher.dispatch(
            SearchPlaneQueryIpcRequest::Text(TextQueryRequest {
                syntax: TextQuerySyntax::Native,
                query_text: "needle".to_string(),
                constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                generation: Some(ready_pin()),
                generation_selector: None,
                top_k: 5,
            }),
            &RequestBudgetV1::unbounded(),
        );

        match response {
            SearchPlaneQueryIpcResponse::Text(text) => {
                if text.results.len() != 1 {
                    return Err(format!("expected 1 result, got {}", text.results.len()).into());
                }
            }
            SearchPlaneQueryIpcResponse::Error(err) => {
                return Err(format!(
                    "expected Text response, got Error {} / {}",
                    err.code, err.message
                )
                .into());
            }
            other @ (SearchPlaneQueryIpcResponse::Symbol(_)
            | SearchPlaneQueryIpcResponse::Semantic(_)
            | SearchPlaneQueryIpcResponse::Hybrid(_)
            | SearchPlaneQueryIpcResponse::HybridSeed(_)
            | SearchPlaneQueryIpcResponse::History(_)
            | SearchPlaneQueryIpcResponse::Structural(_)
            | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
            | SearchPlaneQueryIpcResponse::Explain(_)
            | SearchPlaneQueryIpcResponse::ClusterMembershipRead(_)
            | SearchPlaneQueryIpcResponse::RuntimeMetadata(_)) => {
                return Err(format!("expected Text response, got {other:?}").into());
            }
        }

        let search_top_ks = {
            let guard = state
                .lock()
                .map_err(|err| format!("lexical state poisoned: {err}"))?;
            guard.search_top_ks.clone()
        };
        if search_top_ks.as_slice() != [6] {
            return Err(format!(
                "expected searcher.search probe with top_k=6, got {search_top_ks:?}"
            )
            .into());
        }
        Ok(())
    }

    #[test]
    fn symbol_dispatch_returns_symbol_candidates_with_kind_truth() -> TestResult {
        let state = Arc::new(Mutex::new(RecordingLexicalState::default()));
        let dispatcher = SearchPlaneDispatcher::new(
            Arc::new(RecordingLexicalOpener {
                state: Arc::clone(&state),
                results: vec![candidate("sym-hit", 1.0)],
            }),
            Arc::new(RejectSemanticOpener),
            Arc::new(StubRepoMapQueryPort),
            Arc::new(FailClosedStructuralProducer),
            ready_ledger(),
            test_activation_catalog()?,
        );

        let response = dispatcher.dispatch(
            SearchPlaneQueryIpcRequest::Symbol(quanta_index_contract::SymbolQueryRequest {
                syntax: TextQuerySyntax::Native,
                query_text: "type:symbol MySymbol".to_string(),
                constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                generation: Some(ready_pin()),
                generation_selector: None,
                top_k: 3,
            }),
            &RequestBudgetV1::unbounded(),
        );

        match response {
            SearchPlaneQueryIpcResponse::Symbol(symbols) => {
                if symbols.results.len() != 1 {
                    return Err(
                        format!("expected 1 symbol result, got {}", symbols.results.len()).into(),
                    );
                }
                let Some(first) = symbols.results.first() else {
                    return Err("expected one symbol candidate".into());
                };
                if first.candidate_id != "sym-hit"
                    || first.symbol_kind.as_str() != "function"
                    || first.symbol_kind_family != Some(SymbolKindFamily::Callable)
                {
                    return Err(format!("unexpected symbol candidate: {first:?}").into());
                }
            }
            other @ (SearchPlaneQueryIpcResponse::Text(_)
            | SearchPlaneQueryIpcResponse::Semantic(_)
            | SearchPlaneQueryIpcResponse::Hybrid(_)
            | SearchPlaneQueryIpcResponse::HybridSeed(_)
            | SearchPlaneQueryIpcResponse::History(_)
            | SearchPlaneQueryIpcResponse::ClusterMembershipRead(_)
            | SearchPlaneQueryIpcResponse::RuntimeMetadata(_)
            | SearchPlaneQueryIpcResponse::Structural(_)
            | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
            | SearchPlaneQueryIpcResponse::Explain(_)
            | SearchPlaneQueryIpcResponse::Error(_)) => {
                return Err(format!("expected Symbol response, got {other:?}").into());
            }
        }

        let guard = state
            .lock()
            .map_err(|err| format!("lexical state poisoned: {err}"))?;
        if guard.symbol_top_ks.as_slice() != [4] {
            return Err(format!(
                "expected symbol route to probe with top_k=4, got {:?}",
                guard.symbol_top_ks
            )
            .into());
        }
        drop(guard);
        Ok(())
    }

    #[test]
    fn history_dispatch_maps_generation_not_ready_before_lexical_materialization() -> TestResult {
        let dispatcher = history_dispatcher_with_ledger(Arc::new(RwLock::new(Ledger::default())))?;

        let response = dispatcher.dispatch(
            history_query_request("type:commit fix"),
            &RequestBudgetV1::unbounded(),
        );

        let (code, _message) =
            ipc_error_from(response).map_err(Box::<dyn std::error::Error>::from)?;
        if code != ERR_HISTORY_GENERATION_NOT_READY {
            return Err(format!("expected {ERR_HISTORY_GENERATION_NOT_READY}, got {code}").into());
        }
        Ok(())
    }

    #[test]
    fn history_dispatch_maps_producer_unavailable_after_lexical_ready() -> TestResult {
        let dispatcher = history_dispatcher_with_ledger(ready_ledger())?;

        let response = dispatcher.dispatch(
            history_query_request("type:commit fix"),
            &RequestBudgetV1::unbounded(),
        );

        let (code, _message) =
            ipc_error_from(response).map_err(Box::<dyn std::error::Error>::from)?;
        if code != ERR_HISTORY_PRODUCER_UNAVAILABLE {
            return Err(format!("expected {ERR_HISTORY_PRODUCER_UNAVAILABLE}, got {code}").into());
        }
        Ok(())
    }

    #[test]
    fn history_dispatch_maps_shard_unavailable_for_missing_diff_shard() -> TestResult {
        let commit_payload = encode_cbor(&history_commit_record())?;
        let dispatcher = history_dispatcher_with_ledger(ledger_with_history_ops(vec![
            LexicalChannelOp::UpsertCommit(UpsertCommit {
                repo_id: RepoId::new("repo-map-ipc"),
                revision_id: RevisionId::new("rev-map-ipc"),
                generation: ManifestGeneration::new(9),
                payload: commit_payload,
            }),
        ])?)?;

        let response = dispatcher.dispatch(
            history_query_request("type:diff history"),
            &RequestBudgetV1::unbounded(),
        );

        let (code, _message) =
            ipc_error_from(response).map_err(Box::<dyn std::error::Error>::from)?;
        if code != ERR_HISTORY_SHARD_UNAVAILABLE {
            return Err(format!("expected {ERR_HISTORY_SHARD_UNAVAILABLE}, got {code}").into());
        }
        Ok(())
    }

    #[test]
    fn structural_dispatch_routes_happy_path_through_structural_service() -> TestResult {
        let producer = Arc::new(RecordingStructuralProducer::ready_with(vec![
            structural_match_candidate("chunk-tree"),
        ]));
        let dispatcher = structural_dispatcher_with_producer(Arc::clone(&producer))?;

        let response = dispatcher.dispatch(
            SearchPlaneQueryIpcRequest::Structural(quanta_index_contract::StructuralQueryRequest {
                text_query: TextQueryRequest {
                    syntax: TextQuerySyntax::Native,
                    query_text: "match { :[x] }".to_string(),
                    constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                    generation: Some(ready_pin()),
                    generation_selector: None,
                    top_k: 4,
                },
            }),
            &RequestBudgetV1::unbounded(),
        );

        match response {
            SearchPlaneQueryIpcResponse::Structural(results) => {
                if results.generation != ready_pin() {
                    return Err(format!(
                        "expected structural generation {:?}, got {:?}",
                        ready_pin(),
                        results.generation
                    )
                    .into());
                }
                if results.results.len() != 1 {
                    return Err(format!(
                        "expected 1 structural candidate, got {}",
                        results.results.len()
                    )
                    .into());
                }
                let Some(first) = results.results.first() else {
                    return Err("expected structural results to contain one candidate".into());
                };
                if first.candidate_id != "chunk-tree" {
                    return Err(format!("expected candidate_id=chunk-tree, got {first:?}").into());
                }
            }
            other @ (SearchPlaneQueryIpcResponse::Text(_)
            | SearchPlaneQueryIpcResponse::Symbol(_)
            | SearchPlaneQueryIpcResponse::Semantic(_)
            | SearchPlaneQueryIpcResponse::Hybrid(_)
            | SearchPlaneQueryIpcResponse::HybridSeed(_)
            | SearchPlaneQueryIpcResponse::History(_)
            | SearchPlaneQueryIpcResponse::ClusterMembershipRead(_)
            | SearchPlaneQueryIpcResponse::RuntimeMetadata(_)
            | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
            | SearchPlaneQueryIpcResponse::Explain(_)
            | SearchPlaneQueryIpcResponse::Error(_)) => {
                return Err(format!("expected Structural response, got {other:?}").into());
            }
        }
        let consulted = producer.readiness_calls.load(Ordering::SeqCst);
        if consulted != 1 {
            return Err(format!(
                "expected structural readiness to be consulted exactly once, got {consulted} call(s)"
            )
            .into());
        }
        let executed = producer.execute_calls.load(Ordering::SeqCst);
        if executed != 1 {
            return Err(format!(
                "expected structural execute to be consulted exactly once, got {executed} call(s)"
            )
            .into());
        }
        Ok(())
    }

    #[test]
    fn structural_dispatch_maps_generation_not_ready() -> TestResult {
        let producer = Arc::new(RecordingStructuralProducer::new(
            StructuralReadiness::GenerationNotReady,
        ));
        let dispatcher = structural_dispatcher_with_producer(Arc::clone(&producer))?;

        let response = dispatcher.dispatch(
            SearchPlaneQueryIpcRequest::Structural(quanta_index_contract::StructuralQueryRequest {
                text_query: TextQueryRequest {
                    syntax: TextQuerySyntax::Native,
                    query_text: "match { :[x] }".to_string(),
                    constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                    generation: Some(ready_pin()),
                    generation_selector: None,
                    top_k: 4,
                },
            }),
            &RequestBudgetV1::unbounded(),
        );

        let (code, _message) =
            ipc_error_from(response).map_err(Box::<dyn std::error::Error>::from)?;
        if code != "STR_GENERATION_NOT_READY" {
            return Err(format!("expected STR_GENERATION_NOT_READY, got {code}").into());
        }
        if producer.execute_calls.load(Ordering::SeqCst) != 0 {
            return Err("execute must not run when readiness is GenerationNotReady".into());
        }
        Ok(())
    }

    #[test]
    fn structural_dispatch_maps_shard_unavailable() -> TestResult {
        let producer = Arc::new(RecordingStructuralProducer::new(
            StructuralReadiness::ShardUnavailable,
        ));
        let dispatcher = structural_dispatcher_with_producer(Arc::clone(&producer))?;

        let response = dispatcher.dispatch(
            SearchPlaneQueryIpcRequest::Structural(quanta_index_contract::StructuralQueryRequest {
                text_query: TextQueryRequest {
                    syntax: TextQuerySyntax::Native,
                    query_text: "match { :[x] }".to_string(),
                    constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                    generation: Some(ready_pin()),
                    generation_selector: None,
                    top_k: 4,
                },
            }),
            &RequestBudgetV1::unbounded(),
        );

        let (code, _message) =
            ipc_error_from(response).map_err(Box::<dyn std::error::Error>::from)?;
        if code != "STR_SHARD_UNAVAILABLE" {
            return Err(format!("expected STR_SHARD_UNAVAILABLE, got {code}").into());
        }
        if producer.execute_calls.load(Ordering::SeqCst) != 0 {
            return Err("execute must not run when readiness is ShardUnavailable".into());
        }
        Ok(())
    }

    #[test]
    fn structural_dispatch_maps_lang_not_supported() -> TestResult {
        let producer = Arc::new(RecordingStructuralProducer::ready_with_error(
            StructuralError::LangNotSupported("java".to_string()),
        ));
        let dispatcher = structural_dispatcher_with_producer(Arc::clone(&producer))?;

        let response = dispatcher.dispatch(
            SearchPlaneQueryIpcRequest::Structural(quanta_index_contract::StructuralQueryRequest {
                text_query: TextQueryRequest {
                    syntax: TextQuerySyntax::Native,
                    query_text: "lang:java match { :[x] }".to_string(),
                    constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                    generation: Some(ready_pin()),
                    generation_selector: None,
                    top_k: 4,
                },
            }),
            &RequestBudgetV1::unbounded(),
        );

        let (code, _message) =
            ipc_error_from(response).map_err(Box::<dyn std::error::Error>::from)?;
        if code != "STR_LANG_NOT_SUPPORTED" {
            return Err(format!("expected STR_LANG_NOT_SUPPORTED, got {code}").into());
        }
        Ok(())
    }

    #[test]
    fn structural_dispatch_routes_repo_and_file_filters_to_producer() -> TestResult {
        let producer = Arc::new(RecordingStructuralProducer::ready_with(vec![
            structural_match_candidate("chunk-tree"),
        ]));
        let dispatcher = structural_dispatcher_with_producer(Arc::clone(&producer))?;

        let response = dispatcher.dispatch(
            SearchPlaneQueryIpcRequest::Structural(quanta_index_contract::StructuralQueryRequest {
                text_query: TextQueryRequest {
                    syntax: TextQuerySyntax::Native,
                    query_text: "repo:repo-map-ipc file:src/lib.rs match { :[x] }".to_string(),
                    constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                    generation: Some(ready_pin()),
                    generation_selector: None,
                    top_k: 4,
                },
            }),
            &RequestBudgetV1::unbounded(),
        );

        match response {
            SearchPlaneQueryIpcResponse::Structural(results) => {
                if results.generation != ready_pin() {
                    return Err(format!(
                        "expected structural generation {:?}, got {:?}",
                        ready_pin(),
                        results.generation
                    )
                    .into());
                }
                if results.results.len() != 1 {
                    return Err(format!(
                        "expected 1 structural candidate, got {}",
                        results.results.len()
                    )
                    .into());
                }
            }
            other @ (SearchPlaneQueryIpcResponse::Text(_)
            | SearchPlaneQueryIpcResponse::Symbol(_)
            | SearchPlaneQueryIpcResponse::Semantic(_)
            | SearchPlaneQueryIpcResponse::Hybrid(_)
            | SearchPlaneQueryIpcResponse::HybridSeed(_)
            | SearchPlaneQueryIpcResponse::History(_)
            | SearchPlaneQueryIpcResponse::ClusterMembershipRead(_)
            | SearchPlaneQueryIpcResponse::RuntimeMetadata(_)
            | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
            | SearchPlaneQueryIpcResponse::Explain(_)
            | SearchPlaneQueryIpcResponse::Error(_)) => {
                return Err(format!("expected Structural response, got {other:?}").into());
            }
        }
        if producer.readiness_calls.load(Ordering::SeqCst) != 1 {
            return Err("producer readiness must run for executable structural filters".into());
        }
        if producer.execute_calls.load(Ordering::SeqCst) != 1 {
            return Err("producer execute must run for executable structural filters".into());
        }
        Ok(())
    }

    #[test]
    fn structural_dispatch_rejects_non_executable_filters_before_consulting_producer() -> TestResult
    {
        let producer = Arc::new(RecordingStructuralProducer::ready_with(vec![
            structural_match_candidate("chunk-tree"),
        ]));
        let dispatcher = structural_dispatcher_with_producer(Arc::clone(&producer))?;

        let response = dispatcher.dispatch(
            SearchPlaneQueryIpcRequest::Structural(quanta_index_contract::StructuralQueryRequest {
                text_query: TextQueryRequest {
                    syntax: TextQuerySyntax::Native,
                    query_text: "select:repo match { :[x] }".to_string(),
                    constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                    generation: Some(ready_pin()),
                    generation_selector: None,
                    top_k: 4,
                },
            }),
            &RequestBudgetV1::unbounded(),
        );

        let (code, message) =
            ipc_error_from(response).map_err(Box::<dyn std::error::Error>::from)?;
        if code != "STR_INVALID_REQUEST" {
            return Err(format!("expected STR_INVALID_REQUEST, got {code}").into());
        }
        if !message.contains("filter `select` is not executable") {
            return Err(format!("expected select-filter rejection message, got {message}").into());
        }
        if producer.readiness_calls.load(Ordering::SeqCst) != 0 {
            return Err("producer readiness must not run for invalid structural filters".into());
        }
        if producer.execute_calls.load(Ordering::SeqCst) != 0 {
            return Err("producer execute must not run for invalid structural filters".into());
        }
        Ok(())
    }

    #[test]
    fn structural_dispatch_routes_sourcegraph_structural_subset_to_producer() -> TestResult {
        let producer = Arc::new(RecordingStructuralProducer::ready_with(vec![
            structural_match_candidate("chunk-tree"),
        ]));
        let dispatcher = structural_dispatcher_with_producer(Arc::clone(&producer))?;

        let response = dispatcher.dispatch(SearchPlaneQueryIpcRequest::Structural(
            quanta_index_contract::StructuralQueryRequest {
                text_query: TextQueryRequest {
                    syntax: TextQuerySyntax::Sourcegraph,
                    query_text:
                        r#"repo:repo-map-ipc path:src/lib.rs lang:rust patterntype:structural "function_item { { identifier :[x] } }""#
                            .to_string(),
                    constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                    generation: Some(ready_pin()),
                    generation_selector: None,
                    top_k: 4,
                },
            },
        ), &RequestBudgetV1::unbounded());

        match response {
            SearchPlaneQueryIpcResponse::Structural(results) => {
                if results.generation != ready_pin() {
                    return Err(format!(
                        "expected structural generation {:?}, got {:?}",
                        ready_pin(),
                        results.generation
                    )
                    .into());
                }
                if results.results.len() != 1 {
                    return Err(format!(
                        "expected 1 structural candidate from SG structural route, got {}",
                        results.results.len()
                    )
                    .into());
                }
            }
            other @ (SearchPlaneQueryIpcResponse::Text(_)
            | SearchPlaneQueryIpcResponse::Symbol(_)
            | SearchPlaneQueryIpcResponse::Semantic(_)
            | SearchPlaneQueryIpcResponse::Hybrid(_)
            | SearchPlaneQueryIpcResponse::HybridSeed(_)
            | SearchPlaneQueryIpcResponse::History(_)
            | SearchPlaneQueryIpcResponse::ClusterMembershipRead(_)
            | SearchPlaneQueryIpcResponse::RuntimeMetadata(_)
            | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
            | SearchPlaneQueryIpcResponse::Explain(_)
            | SearchPlaneQueryIpcResponse::Error(_)) => {
                return Err(format!(
                    "expected Structural response for SG structural route, got {other:?}"
                )
                .into());
            }
        }
        if producer.readiness_calls.load(Ordering::SeqCst) != 1 {
            return Err("producer readiness must run for SG structural subset".into());
        }
        if producer.execute_calls.load(Ordering::SeqCst) != 1 {
            return Err("producer execute must run for SG structural subset".into());
        }
        Ok(())
    }

    #[test]
    fn structural_dispatch_rejects_typed_hole_kind_with_exact_code() -> TestResult {
        let producer = Arc::new(RecordingStructuralProducer::ready_with(vec![
            structural_match_candidate("chunk-tree"),
        ]));
        let dispatcher = structural_dispatcher_with_producer(Arc::clone(&producer))?;

        let response = dispatcher.dispatch(
            SearchPlaneQueryIpcRequest::Structural(quanta_index_contract::StructuralQueryRequest {
                text_query: TextQueryRequest {
                    syntax: TextQuerySyntax::Native,
                    query_text: "match { function_item { { :[name.lambda] } } }".to_string(),
                    constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                    generation: Some(ready_pin()),
                    generation_selector: None,
                    top_k: 4,
                },
            }),
            &RequestBudgetV1::unbounded(),
        );

        let (code, message) =
            ipc_error_from(response).map_err(Box::<dyn std::error::Error>::from)?;
        if code != "STR_HOLE_KIND_UNSUPPORTED" {
            return Err(format!("expected STR_HOLE_KIND_UNSUPPORTED, got {code}").into());
        }
        if !message.contains("typed hole kind `lambda`") {
            return Err(format!("expected typed-hole rejection message, got {message}").into());
        }
        if producer.readiness_calls.load(Ordering::SeqCst) != 0 {
            return Err("producer readiness must not run for typed-hole rejection".into());
        }
        if producer.execute_calls.load(Ordering::SeqCst) != 0 {
            return Err("producer execute must not run for typed-hole rejection".into());
        }
        Ok(())
    }

    #[test]
    fn structural_dispatch_executes_structural_boolean_and_with_canonical_projection() -> TestResult
    {
        let producer = Arc::new(PatternRoutingStructuralProducer::new());
        let dispatcher = structural_dispatcher_with_producer(Arc::clone(&producer))?;

        let response = dispatcher.dispatch(
            SearchPlaneQueryIpcRequest::Structural(quanta_index_contract::StructuralQueryRequest {
                text_query: TextQueryRequest {
                    syntax: TextQuerySyntax::Native,
                    query_text: "match { alpha } AND match { beta }".to_string(),
                    constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                    generation: Some(ready_pin()),
                    generation_selector: None,
                    top_k: 10,
                },
            }),
            &RequestBudgetV1::unbounded(),
        );

        match response {
            SearchPlaneQueryIpcResponse::Structural(results) => {
                if results.results.len() != 1 {
                    return Err(format!(
                        "expected 1 structural candidate from boolean AND, got {}",
                        results.results.len()
                    )
                    .into());
                }
                let candidate = results
                    .results
                    .first()
                    .ok_or_else(|| "missing structural candidate after size check".to_string())?;
                if candidate.candidate_id != "chunk-shared" {
                    return Err(format!("expected chunk-shared, got {candidate:?}").into());
                }
                let start_bytes: Vec<u32> = candidate
                    .bindings
                    .iter()
                    .map(|binding| binding.start_byte)
                    .collect();
                if start_bytes != vec![5, 30] {
                    return Err(format!(
                        "expected canonical merged bindings [5, 30], got {start_bytes:?}"
                    )
                    .into());
                }
            }
            other @ (SearchPlaneQueryIpcResponse::Text(_)
            | SearchPlaneQueryIpcResponse::Symbol(_)
            | SearchPlaneQueryIpcResponse::Semantic(_)
            | SearchPlaneQueryIpcResponse::Hybrid(_)
            | SearchPlaneQueryIpcResponse::HybridSeed(_)
            | SearchPlaneQueryIpcResponse::History(_)
            | SearchPlaneQueryIpcResponse::ClusterMembershipRead(_)
            | SearchPlaneQueryIpcResponse::RuntimeMetadata(_)
            | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
            | SearchPlaneQueryIpcResponse::Explain(_)
            | SearchPlaneQueryIpcResponse::Error(_)) => {
                return Err(format!("expected Structural response, got {other:?}").into());
            }
        }

        if producer.readiness_calls.load(Ordering::SeqCst) != 2 {
            return Err("boolean AND should consult readiness once per structural leaf".into());
        }
        if producer.execute_calls.load(Ordering::SeqCst) != 2 {
            return Err("boolean AND should execute once per structural leaf".into());
        }
        let scopes = producer.recorded_scopes()?;
        if scopes
            != vec![
                None,
                Some(vec!["chunk-a".to_string(), "chunk-shared".to_string()]),
            ]
        {
            return Err(format!("unexpected AND candidate scopes: {scopes:?}").into());
        }
        Ok(())
    }

    #[test]
    fn structural_dispatch_executes_structural_boolean_or() -> TestResult {
        let producer = Arc::new(PatternRoutingStructuralProducer::new());
        let dispatcher = structural_dispatcher_with_producer(Arc::clone(&producer))?;

        let response = dispatcher.dispatch(
            SearchPlaneQueryIpcRequest::Structural(quanta_index_contract::StructuralQueryRequest {
                text_query: TextQueryRequest {
                    syntax: TextQuerySyntax::Native,
                    query_text: "match { alpha } OR match { beta }".to_string(),
                    constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                    generation: Some(ready_pin()),
                    generation_selector: None,
                    top_k: 10,
                },
            }),
            &RequestBudgetV1::unbounded(),
        );

        match response {
            SearchPlaneQueryIpcResponse::Structural(results) => {
                let ids: Vec<&str> = results
                    .results
                    .iter()
                    .map(|candidate| candidate.candidate_id.as_str())
                    .collect();
                if ids != vec!["chunk-a", "chunk-shared"] {
                    return Err(
                        format!("expected OR ids [chunk-a, chunk-shared], got {ids:?}").into(),
                    );
                }
            }
            other @ (SearchPlaneQueryIpcResponse::Text(_)
            | SearchPlaneQueryIpcResponse::Symbol(_)
            | SearchPlaneQueryIpcResponse::Semantic(_)
            | SearchPlaneQueryIpcResponse::Hybrid(_)
            | SearchPlaneQueryIpcResponse::HybridSeed(_)
            | SearchPlaneQueryIpcResponse::History(_)
            | SearchPlaneQueryIpcResponse::ClusterMembershipRead(_)
            | SearchPlaneQueryIpcResponse::RuntimeMetadata(_)
            | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
            | SearchPlaneQueryIpcResponse::Explain(_)
            | SearchPlaneQueryIpcResponse::Error(_)) => {
                return Err(format!("expected Structural response, got {other:?}").into());
            }
        }

        if producer.execute_calls.load(Ordering::SeqCst) != 2 {
            return Err("boolean OR should execute once per structural leaf".into());
        }
        Ok(())
    }

    #[test]
    fn structural_dispatch_executes_bounded_not() -> TestResult {
        let producer = Arc::new(PatternRoutingStructuralProducer::new());
        let dispatcher = structural_dispatcher_with_producer(Arc::clone(&producer))?;

        let response = dispatcher.dispatch(
            SearchPlaneQueryIpcRequest::Structural(quanta_index_contract::StructuralQueryRequest {
                text_query: TextQueryRequest {
                    syntax: TextQuerySyntax::Native,
                    query_text: "match { alpha } AND NOT match { gamma }".to_string(),
                    constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                    generation: Some(ready_pin()),
                    generation_selector: None,
                    top_k: 10,
                },
            }),
            &RequestBudgetV1::unbounded(),
        );

        match response {
            SearchPlaneQueryIpcResponse::Structural(results) => {
                let ids: Vec<&str> = results
                    .results
                    .iter()
                    .map(|candidate| candidate.candidate_id.as_str())
                    .collect();
                if ids != vec!["chunk-shared"] {
                    return Err(format!(
                        "expected bounded NOT to retain only chunk-shared, got {ids:?}"
                    )
                    .into());
                }
            }
            other @ (SearchPlaneQueryIpcResponse::Text(_)
            | SearchPlaneQueryIpcResponse::Symbol(_)
            | SearchPlaneQueryIpcResponse::Semantic(_)
            | SearchPlaneQueryIpcResponse::Hybrid(_)
            | SearchPlaneQueryIpcResponse::HybridSeed(_)
            | SearchPlaneQueryIpcResponse::History(_)
            | SearchPlaneQueryIpcResponse::ClusterMembershipRead(_)
            | SearchPlaneQueryIpcResponse::RuntimeMetadata(_)
            | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
            | SearchPlaneQueryIpcResponse::Explain(_)
            | SearchPlaneQueryIpcResponse::Error(_)) => {
                return Err(format!("expected Structural response, got {other:?}").into());
            }
        }

        if producer.execute_calls.load(Ordering::SeqCst) != 2 {
            return Err("bounded NOT should execute once per structural leaf".into());
        }
        let scopes = producer.recorded_scopes()?;
        if scopes
            != vec![
                None,
                Some(vec!["chunk-a".to_string(), "chunk-shared".to_string()]),
            ]
        {
            return Err(format!("unexpected bounded-NOT candidate scopes: {scopes:?}").into());
        }
        Ok(())
    }

    #[test]
    fn structural_dispatch_memoizes_identical_leaf_execution() -> TestResult {
        let producer = Arc::new(PatternRoutingStructuralProducer::new());
        let dispatcher = structural_dispatcher_with_producer(Arc::clone(&producer))?;

        let response = dispatcher.dispatch(
            SearchPlaneQueryIpcRequest::Structural(quanta_index_contract::StructuralQueryRequest {
                text_query: TextQueryRequest {
                    syntax: TextQuerySyntax::Native,
                    query_text: "match { alpha } OR match { alpha }".to_string(),
                    constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                    generation: Some(ready_pin()),
                    generation_selector: None,
                    top_k: 10,
                },
            }),
            &RequestBudgetV1::unbounded(),
        );

        match response {
            SearchPlaneQueryIpcResponse::Structural(results) => {
                let ids: Vec<&str> = results
                    .results
                    .iter()
                    .map(|candidate| candidate.candidate_id.as_str())
                    .collect();
                if ids != vec!["chunk-a", "chunk-shared"] {
                    return Err(format!(
                        "expected memoized OR ids [chunk-a, chunk-shared], got {ids:?}"
                    )
                    .into());
                }
            }
            other @ (SearchPlaneQueryIpcResponse::Text(_)
            | SearchPlaneQueryIpcResponse::Symbol(_)
            | SearchPlaneQueryIpcResponse::Semantic(_)
            | SearchPlaneQueryIpcResponse::Hybrid(_)
            | SearchPlaneQueryIpcResponse::HybridSeed(_)
            | SearchPlaneQueryIpcResponse::History(_)
            | SearchPlaneQueryIpcResponse::ClusterMembershipRead(_)
            | SearchPlaneQueryIpcResponse::RuntimeMetadata(_)
            | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
            | SearchPlaneQueryIpcResponse::Explain(_)
            | SearchPlaneQueryIpcResponse::Error(_)) => {
                return Err(format!("expected Structural response, got {other:?}").into());
            }
        }

        if producer.readiness_calls.load(Ordering::SeqCst) != 1 {
            return Err("identical structural leaves should consult readiness once".into());
        }
        if producer.execute_calls.load(Ordering::SeqCst) != 1 {
            return Err("identical structural leaves should execute once".into());
        }
        let scopes = producer.recorded_scopes()?;
        if scopes != vec![None] {
            return Err(format!("unexpected memoized candidate scopes: {scopes:?}").into());
        }
        Ok(())
    }

    #[test]
    fn structural_dispatch_executes_mixed_lexical_and_structural_and() -> TestResult {
        let producer = Arc::new(PatternRoutingStructuralProducer::new());
        let lex_opener = Arc::new(RecordingLexicalOpener {
            state: Arc::new(Mutex::new(RecordingLexicalState::default())),
            results: vec![
                recording_lexical_candidate("chunk-a"),
                recording_lexical_candidate("chunk-shared"),
            ],
        });
        let dispatcher = structural_dispatcher_mixed(
            Arc::clone(&producer),
            lex_opener,
            ready_ledger_with_structural_boolean_chunks(),
        )?;

        let response = dispatcher.dispatch(
            SearchPlaneQueryIpcRequest::Structural(quanta_index_contract::StructuralQueryRequest {
                text_query: TextQueryRequest {
                    syntax: TextQuerySyntax::Native,
                    query_text: "needle AND match { alpha }".to_string(),
                    constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                    generation: Some(ready_pin()),
                    generation_selector: None,
                    top_k: 10,
                },
            }),
            &RequestBudgetV1::unbounded(),
        );

        match response {
            SearchPlaneQueryIpcResponse::Structural(results) => {
                let mut ids = results
                    .results
                    .iter()
                    .map(|candidate| candidate.candidate_id.as_str())
                    .collect::<Vec<_>>();
                ids.sort_unstable();
                if ids != ["chunk-a", "chunk-shared"] {
                    return Err(format!(
                        "expected mixed AND to keep chunk-a and chunk-shared, got {ids:?}"
                    )
                    .into());
                }
                let shared = results
                    .results
                    .iter()
                    .find(|candidate| candidate.candidate_id == "chunk-shared")
                    .ok_or_else(|| "missing chunk-shared structural binding".to_string())?;
                let start_bytes: Vec<u32> = shared
                    .bindings
                    .iter()
                    .map(|binding| binding.start_byte)
                    .collect();
                if start_bytes != [5, 20] {
                    return Err(format!(
                        "expected canonical merged bindings [5, 20], got {start_bytes:?}"
                    )
                    .into());
                }
            }
            other @ (SearchPlaneQueryIpcResponse::Text(_)
            | SearchPlaneQueryIpcResponse::Symbol(_)
            | SearchPlaneQueryIpcResponse::Semantic(_)
            | SearchPlaneQueryIpcResponse::Hybrid(_)
            | SearchPlaneQueryIpcResponse::HybridSeed(_)
            | SearchPlaneQueryIpcResponse::History(_)
            | SearchPlaneQueryIpcResponse::ClusterMembershipRead(_)
            | SearchPlaneQueryIpcResponse::RuntimeMetadata(_)
            | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
            | SearchPlaneQueryIpcResponse::Explain(_)
            | SearchPlaneQueryIpcResponse::Error(_)) => {
                return Err(format!("expected Structural response, got {other:?}").into());
            }
        }

        if producer.readiness_calls.load(Ordering::SeqCst) != 1 {
            return Err("mixed AND should consult structural readiness once".into());
        }
        if producer.execute_calls.load(Ordering::SeqCst) != 1 {
            return Err("mixed AND should execute structural leaf once".into());
        }
        Ok(())
    }

    #[test]
    fn structural_dispatch_executes_pure_negative_root_from_pinned_universe() -> TestResult {
        let producer = Arc::new(PatternRoutingStructuralProducer::new());
        let dispatcher = structural_dispatcher_with_producer_and_ledger(
            Arc::clone(&producer),
            ready_ledger_with_structural_boolean_chunks(),
        )?;

        let response = dispatcher.dispatch(
            SearchPlaneQueryIpcRequest::Structural(quanta_index_contract::StructuralQueryRequest {
                text_query: TextQueryRequest {
                    syntax: TextQuerySyntax::Native,
                    query_text: "NOT match { alpha }".to_string(),
                    constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                    generation: Some(ready_pin()),
                    generation_selector: None,
                    top_k: 10,
                },
            }),
            &RequestBudgetV1::unbounded(),
        );

        match response {
            SearchPlaneQueryIpcResponse::Structural(results) => {
                if results.results.len() != 1 {
                    return Err(format!(
                        "expected 1 pure-negative survivor, got {:?}",
                        results.results
                    )
                    .into());
                }
                let candidate = results
                    .results
                    .first()
                    .ok_or_else(|| "missing pure-negative candidate".to_string())?;
                if candidate.candidate_id != "chunk-beta" {
                    return Err(format!("expected chunk-beta, got {candidate:?}").into());
                }
                if !candidate.bindings.is_empty() {
                    return Err(
                        "pure-negative universe placeholder must not invent bindings".into(),
                    );
                }
            }
            other @ (SearchPlaneQueryIpcResponse::Text(_)
            | SearchPlaneQueryIpcResponse::Symbol(_)
            | SearchPlaneQueryIpcResponse::Semantic(_)
            | SearchPlaneQueryIpcResponse::Hybrid(_)
            | SearchPlaneQueryIpcResponse::HybridSeed(_)
            | SearchPlaneQueryIpcResponse::History(_)
            | SearchPlaneQueryIpcResponse::ClusterMembershipRead(_)
            | SearchPlaneQueryIpcResponse::RuntimeMetadata(_)
            | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
            | SearchPlaneQueryIpcResponse::Explain(_)
            | SearchPlaneQueryIpcResponse::Error(_)) => {
                return Err(format!("expected Structural response, got {other:?}").into());
            }
        }

        if producer.readiness_calls.load(Ordering::SeqCst) != 1 {
            return Err("pure-negative root should consult structural readiness once".into());
        }
        if producer.execute_calls.load(Ordering::SeqCst) != 1 {
            return Err("pure-negative root should execute inner structural leaf once".into());
        }
        Ok(())
    }
}

/// QI-BB-023 — history pages are in recency order, exact, and keyset-paged.
#[cfg(test)]
mod history_page_tests {
    use std::collections::BTreeSet;

    use quanta_index_contract::DiffHunkSide;
    use quanta_index_contract::lex::{CommitRecord, CommitSha, DiffHunkRecord};
    use quanta_index_contract::{
        HistoryCursor, HistoryDiffHunkUpsert, HistoryIngestBatch, LQ_VERSION_TAG, LqExpr, LqFilter,
        LqLeaf, LqOptions, LqQuery, LqSpan, LqType, ManifestGeneration, RepoId, RevisionId,
    };
    use quanta_index_core::CoreError;

    use super::{HistoryPage, execute_history_query};
    use crate::readiness::{HistoryAuthorityState, Ledger};

    type TestRes = Result<(), Box<dyn std::error::Error>>;

    fn sha(byte: u8) -> CommitSha {
        CommitSha::from_bytes([byte; 20])
    }

    fn commit(sha_byte: u8, committer_time_ms: u64, message: &str) -> CommitRecord {
        CommitRecord {
            wire_version: 1,
            sha: sha(sha_byte),
            parents: Vec::new(),
            author_time_ms: committer_time_ms,
            committer_time_ms,
            applied_at_ms: committer_time_ms,
            author: "alice".into(),
            author_name: None,
            author_email: None,
            committer: "alice".into(),
            committer_name: None,
            committer_email: None,
            message: message.into(),
            is_merge: false,
            tags: Vec::new(),
        }
    }

    fn hunk(sha_byte: u8, path: &str) -> HistoryDiffHunkUpsert {
        HistoryDiffHunkUpsert {
            commit_sha: sha(sha_byte),
            file_path: path.into(),
            record: DiffHunkRecord {
                wire_version: 1,
                hunk_header: "@@ -1 +1 @@".into(),
                side: DiffHunkSide::After,
                added_text: "fix line".into(),
                removed_text: "".into(),
                touched_text: "fix line".into(),
                byte_start: 0,
                byte_end: 8,
            },
        }
    }

    /// Five matching commits whose sha order is the reverse of their time
    /// order, plus one that does not match.
    fn state(
        commits: Vec<CommitRecord>,
        hunks: Vec<HistoryDiffHunkUpsert>,
    ) -> Result<HistoryAuthorityState, CoreError> {
        let mut ledger = Ledger::new();
        ledger.apply_history_batch(&HistoryIngestBatch {
            repo_id: RepoId::new("r"),
            revision_id: RevisionId::new("rev"),
            generation: ManifestGeneration::new(1),
            manifest_digest: None,
            batch_digest: "history-order".to_string(),
            commits,
            refs: Vec::new(),
            tags: Vec::new(),
            diff_hunks: hunks,
        })?;
        ledger
            .history_state(
                &RepoId::new("r"),
                &RevisionId::new("rev"),
                ManifestGeneration::new(1),
            )
            .cloned()
            .ok_or_else(|| CoreError::Storage("history state missing".to_string()))
    }

    fn reversed_state() -> Result<HistoryAuthorityState, CoreError> {
        state(
            vec![
                commit(5, 100, "fix one"),
                commit(4, 200, "fix two"),
                commit(3, 300, "fix three"),
                commit(2, 400, "fix four"),
                commit(1, 500, "fix five"),
                commit(9, 600, "unrelated"),
            ],
            Vec::new(),
        )
    }

    fn query(kind: LqType) -> LqQuery {
        LqQuery {
            lq_version: LQ_VERSION_TAG,
            expr: LqExpr::Leaf(LqLeaf::Keyword("fix".to_string())),
            filters: vec![LqFilter::Type { kind }],
            options: LqOptions::defaults(),
            directives: Vec::new(),
            source_span: LqSpan::eof(0),
        }
    }

    fn page(
        state: &HistoryAuthorityState,
        kind: LqType,
        top_k: u32,
        cursor: Option<&HistoryCursor>,
    ) -> Result<HistoryPage, CoreError> {
        execute_history_query(&query(kind), state, top_k, cursor)
    }

    #[test]
    fn top_k_returns_the_newest_matches_not_the_smallest_shas() -> TestRes {
        let state = reversed_state()?;
        let page = page(&state, LqType::Commit, 2, None)?;
        let shas: Vec<CommitSha> = page.commits.iter().map(|commit| commit.sha).collect();
        if shas != vec![sha(1), sha(2)] {
            return Err(format!("top-2 must be the two newest matches, got {shas:?}").into());
        }
        if page.window.returned() != 2
            || page.window.candidate_count() != quanta_index_contract::CandidateCountV1::Exact(5)
            || !page.window.has_more()
        {
            return Err(format!("window drifted: {:?}", page.window).into());
        }
        if page.examined != 6 {
            return Err(format!("every record is examined once, saw {}", page.examined).into());
        }
        let cursor = page
            .next_cursor
            .ok_or("a page with more must carry a cursor")?;
        if cursor.sha != sha(2) || cursor.committer_time_ms != 400 || cursor.file_path.is_some() {
            return Err(format!("the cursor names the last row, got {cursor:?}").into());
        }
        Ok(())
    }

    #[test]
    fn pages_partition_the_matches_in_order_without_gaps_or_overlap() -> TestRes {
        let state = reversed_state()?;
        let mut cursor: Option<HistoryCursor> = None;
        let mut seen: Vec<CommitSha> = Vec::new();
        let mut pages = 0_u32;
        loop {
            let page = page(&state, LqType::Commit, 2, cursor.as_ref())?;
            pages = pages.saturating_add(1);
            seen.extend(page.commits.iter().map(|commit| commit.sha));
            if page.window.candidate_count()
                != quanta_index_contract::CandidateCountV1::Exact(
                    5_u64
                        .saturating_sub(u64::try_from(seen.len())?)
                        .saturating_add(u64::try_from(page.commits.len())?),
                )
            {
                return Err(format!(
                    "each page counts exactly the matches after its cursor: {:?}",
                    page.window
                )
                .into());
            }
            match page.next_cursor {
                Some(next) if page.window.has_more() => cursor = Some(next),
                None if !page.window.has_more() => break,
                other => return Err(format!("has_more and cursor disagree: {other:?}").into()),
            }
            if pages > 10 {
                return Err("pagination did not terminate".into());
            }
        }
        if seen != vec![sha(1), sha(2), sha(3), sha(4), sha(5)] {
            return Err(
                format!("pages must partition the matches in recency order, got {seen:?}").into(),
            );
        }
        if pages != 3 {
            return Err(
                format!("five matches at two per page is three pages, took {pages}").into(),
            );
        }
        let distinct: BTreeSet<CommitSha> = seen.iter().copied().collect();
        if distinct.len() != seen.len() {
            return Err("a commit appeared on two pages".into());
        }
        Ok(())
    }

    #[test]
    fn equal_times_break_ties_by_sha_ascending() -> TestRes {
        let state = state(
            vec![
                commit(7, 100, "fix c"),
                commit(3, 100, "fix a"),
                commit(5, 100, "fix b"),
                commit(1, 50, "fix older"),
            ],
            Vec::new(),
        )?;
        let page = page(&state, LqType::Commit, 10, None)?;
        let shas: Vec<CommitSha> = page.commits.iter().map(|commit| commit.sha).collect();
        if shas != vec![sha(3), sha(5), sha(7), sha(1)] {
            return Err(format!("ties break by sha ascending, older last: {shas:?}").into());
        }
        if page.window.has_more() || page.next_cursor.is_some() {
            return Err("a complete page carries no continuation".into());
        }
        Ok(())
    }

    #[test]
    fn diff_pages_order_by_commit_recency_then_path_and_refuse_a_commit_cursor() -> TestRes {
        let state = state(
            vec![commit(2, 100, "fix old"), commit(1, 200, "fix new")],
            vec![
                hunk(2, "b.rs"),
                hunk(2, "a.rs"),
                hunk(1, "z.rs"),
                hunk(1, "m.rs"),
            ],
        )?;
        let first = page(&state, LqType::Diff, 3, None)?;
        let paths: Vec<&str> = first
            .diffs
            .iter()
            .map(|diff| diff.repo_relative_path.as_str())
            .collect();
        if paths != vec!["m.rs", "z.rs", "a.rs"] {
            return Err(format!("diffs order by commit recency then path, got {paths:?}").into());
        }
        let cursor = first.next_cursor.ok_or("more diffs remain")?;
        if cursor.file_path.as_deref() != Some("a.rs") || cursor.sha != sha(2) {
            return Err(format!("the diff cursor names the last hunk, got {cursor:?}").into());
        }
        let second = page(&state, LqType::Diff, 3, Some(&cursor))?;
        let paths: Vec<&str> = second
            .diffs
            .iter()
            .map(|diff| diff.repo_relative_path.as_str())
            .collect();
        if paths != vec!["b.rs"] || second.window.has_more() {
            return Err(format!("the second page holds the rest: {paths:?}").into());
        }
        // A commit cursor cannot position a diff page, nor the reverse.
        let commit_cursor = HistoryCursor {
            committer_time_ms: 200,
            sha: sha(1),
            file_path: None,
        };
        match page(&state, LqType::Diff, 3, Some(&commit_cursor)) {
            Err(CoreError::InvalidContract(_)) => {}
            other => {
                return Err(format!("a commit cursor on a diff page answered {other:?}").into());
            }
        }
        match page(&state, LqType::Commit, 3, Some(&cursor)) {
            Err(CoreError::InvalidContract(_)) => {}
            other => {
                return Err(format!("a diff cursor on a commit page answered {other:?}").into());
            }
        }
        Ok(())
    }
}
