//! The `SearchPlaneDispatcher` type: composition, IPC request fan-out under
//! the request budget, and per-route metric emission. Route bodies live
//! under `routes/`.

use std::sync::{Arc, OnceLock, RwLock};
use std::time::Instant;

use quanta_index_contract::{
    ActiveGenerationResolutionV1, ClusterMembershipBatchReadRequestV1, CurrentGenerationRequest,
    EarlyStopReason, GenerationPin, HistoryQueryRequest, HybridQueryRequest,
    HybridSeedQueryRequest, RepoMapQueryRequest, RuntimeMetadataQueryRequest,
    SearchCorpusActiveHeadV1, SearchPlaneExplainQueryRequest, SearchPlaneQueryIpcRequest,
    SearchPlaneQueryIpcResponse, SearchPlaneTrackKind, SemanticQueryRequest,
    SemanticWorkBoundedQueryRequestV1, SemanticWorkBoundedQueryResponseV1, StructuralQueryRequest,
    SymbolQueryRequest, TextQueryRequest,
};
use quanta_index_core::domains::structural::StructuralProducerPort;
use quanta_index_core::{
    CoreError, ExplainQueryPort, HybridFetchFloorPolicy, HybridQueryPort, LexicalIndexOpenPort,
    QueryRouteV1, RepoMapSnapshotAcquirePort, RequestBudgetV1, SemanticIndexOpenPort,
    SemanticQueryPort,
};
use quanta_index_lq_obs::{Dimensions, MetricKind, MetricSample};

use crate::history_text::HistoryTextIndexParts;
use crate::observability::{NoopQueryObsSink, QueryObsSink};
use crate::query_dispatcher::continuation::CursorAuthorityV2;
use crate::query_dispatcher::cursor_key::CursorKeyStore;
use crate::query_dispatcher::errors::core_error_to_ipc;
use crate::query_dispatcher::metrics::{
    QueryRoute, classify_error_metric_name, elapsed_millis_metric, examined_candidates_metric,
    interruption_route_suffix, metric_count_value,
};
use crate::query_dispatcher::response_budget::ResponsePayloadBudget;
use crate::query_dispatcher::stage_timing::QueryStageObservationPolicy;
use crate::query_embedder::{HashingQueryTextEmbedder, QueryTextEmbedderPort};
use crate::{
    ActivationCatalog, Ledger, SEARCH_OWNED_SEMANTIC_DIMENSION, SnapshotRegistries,
    SnapshotRegistryPolicy,
};

pub struct SearchPlaneDispatcher {
    pub(super) lex_opener: Arc<dyn LexicalIndexOpenPort + Send + Sync>,
    pub(super) sem_opener: Arc<dyn SemanticIndexOpenPort + Send + Sync>,
    /// Resident opened generations, shared with the ingest side which
    /// invalidates them (QI-BB-001).
    pub(super) snapshots: SnapshotRegistries,
    /// The `RepoMap` snapshot acquisition port (S21-05): the read view
    /// acquires one pinned handle per request; no route holds this port
    /// for execution.
    pub(super) repo_map_snapshots: Arc<dyn RepoMapSnapshotAcquirePort + Send + Sync>,
    /// Structural producer adapter wired by the composition root.
    pub(super) structural_producer: Arc<dyn StructuralProducerPort + Send + Sync>,
    pub(super) ledger: Arc<RwLock<Ledger>>,
    pub(super) activation_catalog: Arc<ActivationCatalog>,
    pub(super) query_embedder: Arc<dyn QueryTextEmbedderPort + Send + Sync>,
    pub(super) obs_sink: Arc<dyn QueryObsSink + Send + Sync>,
    /// The history text index the relevance order scores with (QI-BB-023
    /// follow-up #1).
    ///
    /// Shared with the ingest side, which publishes and retires its
    /// epochs; a plane composed without one refuses that order typed.
    pub(super) history_text: Option<HistoryTextIndexParts>,
    /// How many encoded bytes one ranked lexical page may take before it
    /// is cut and continued by its cursor (QI-BB-005 보완 #5).
    pub(super) response_budget: ResponsePayloadBudget,
    pub(super) query_stage_observation: QueryStageObservationPolicy,
    pub(super) hybrid_fetch_floor: HybridFetchFloorPolicy,
    /// One continuation authority for all pageable routes. Product
    /// composition installs the persistent state-root key before serving;
    /// owner-local composition initializes a process-local authority lazily.
    pub(super) cursor_authority: OnceLock<CursorAuthorityV2>,
}

pub type SearchPlaneQueryService = SearchPlaneDispatcher;
pub type SearchPlaneQueryDispatcher = SearchPlaneDispatcher;

impl SearchPlaneDispatcher {
    #[must_use]
    pub fn new(
        lex_opener: Arc<dyn LexicalIndexOpenPort + Send + Sync>,
        sem_opener: Arc<dyn SemanticIndexOpenPort + Send + Sync>,
        repo_map_snapshots: Arc<dyn RepoMapSnapshotAcquirePort + Send + Sync>,
        structural_producer: Arc<dyn StructuralProducerPort + Send + Sync>,
        ledger: Arc<RwLock<Ledger>>,
        activation_catalog: Arc<ActivationCatalog>,
    ) -> Self {
        Self::new_with_obs(
            lex_opener,
            sem_opener,
            SnapshotRegistries::new(SnapshotRegistryPolicy::DEFAULT),
            repo_map_snapshots,
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
        repo_map_snapshots: Arc<dyn RepoMapSnapshotAcquirePort + Send + Sync>,
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
            repo_map_snapshots,
            structural_producer,
            ledger,
            activation_catalog,
            query_embedder,
            obs_sink,
            history_text: None,
            response_budget: ResponsePayloadBudget::DEFAULT,
            query_stage_observation: QueryStageObservationPolicy::default(),
            hybrid_fetch_floor: HybridFetchFloorPolicy::default(),
            cursor_authority: OnceLock::new(),
        }
    }

    /// Wire the history text index the relevance order scores with.
    #[must_use]
    pub fn with_history_text(mut self, history_text: HistoryTextIndexParts) -> Self {
        self.history_text = Some(history_text);
        self
    }

    /// Cut ranked lexical pages at `budget` encoded bytes instead of the
    /// frame's capacity.
    #[must_use]
    pub const fn with_response_budget(mut self, budget: ResponsePayloadBudget) -> Self {
        self.response_budget = budget;
        self
    }

    /// Select stage instrumentation before sharing the dispatcher. This is not
    /// a sidecar-output switch: disabled routes do not read stage clocks.
    #[must_use]
    pub const fn with_query_stage_observation(
        mut self,
        policy: QueryStageObservationPolicy,
    ) -> Self {
        self.query_stage_observation = policy;
        self
    }

    #[must_use]
    pub const fn query_stage_observation(&self) -> QueryStageObservationPolicy {
        self.query_stage_observation
    }

    /// Select one bounded experimental floor without changing refill or probe rules.
    #[must_use]
    pub const fn with_hybrid_fetch_floor(mut self, floor: HybridFetchFloorPolicy) -> Self {
        self.hybrid_fetch_floor = floor;
        self
    }

    #[must_use]
    pub const fn hybrid_fetch_floor(&self) -> HybridFetchFloorPolicy {
        self.hybrid_fetch_floor
    }

    /// Install the persistent cursor signing key held under the product
    /// state root. This must run before the dispatcher is shared.
    #[must_use]
    pub fn with_cursor_key_store(self, keys: CursorKeyStore) -> Self {
        let _installed = self
            .cursor_authority
            .set(CursorAuthorityV2::persistent(keys));
        self
    }

    pub(super) fn cursors(&self) -> Result<&CursorAuthorityV2, CoreError> {
        if let Some(authority) = self.cursor_authority.get() {
            return Ok(authority);
        }
        let authority = CursorAuthorityV2::process_local()?;
        let _raced = self.cursor_authority.set(authority);
        self.cursor_authority.get().ok_or_else(|| {
            CoreError::Storage("cursor authority initialization did not publish".to_string())
        })
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
            SearchPlaneQueryIpcRequest::ResolveActiveGeneration(req) => {
                self.dispatch_active_resolution(&req, budget)
            }
            SearchPlaneQueryIpcRequest::ResolveLexicalGeneration(req) => {
                self.dispatch_lexical_resolution(&req, budget)
            }
            SearchPlaneQueryIpcRequest::Text(req) => self.dispatch_text(&req, budget),
            SearchPlaneQueryIpcRequest::Symbol(req) => self.dispatch_symbol(req, budget),
            SearchPlaneQueryIpcRequest::Semantic(req) => self.dispatch_semantic(req, budget),
            SearchPlaneQueryIpcRequest::SemanticWorkBoundedV1(req) => {
                self.dispatch_semantic_work_bounded_v1(req, budget)
            }
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

    fn dispatch_active_resolution(
        &self,
        request: &CurrentGenerationRequest,
        budget: &RequestBudgetV1,
    ) -> SearchPlaneQueryIpcResponse {
        self.observed_route(QueryRoute::ActiveResolution, None, || {
            let resolved = budget.checkpoint("active-resolution:entry").and_then(|()| {
                if request.track == SearchPlaneTrackKind::Structural {
                    return Err(CoreError::NotReady(
                        "active-resolution: structural track has no composite active head"
                            .to_string(),
                    ));
                }
                let (generation, activation_token) = self
                    .activation_catalog
                    .active_search_corpus_with_token_v1(&request.repo_id, &request.revision_id)?
                    .ok_or_else(|| {
                        CoreError::NotReady(format!(
                            "active-resolution: no active composite head for repo={} revision={}",
                            request.repo_id.as_str(),
                            request.revision_id.as_str()
                        ))
                    })?;
                let resolution = ActiveGenerationResolutionV1 {
                    track: request.track,
                    head: SearchCorpusActiveHeadV1 {
                        generation: generation.to_contract_v1(),
                        activation_token,
                    },
                };
                resolution.validate_v1().map_err(|error| {
                    CoreError::Storage(format!("active-resolution: invalid catalog head: {error}"))
                })?;
                Ok(resolution)
            });
            match resolved {
                Ok(resolution) => SearchPlaneQueryIpcResponse::ActiveGenerationSnapshot(resolution),
                Err(error) => SearchPlaneQueryIpcResponse::Error(core_error_to_ipc(error)),
            }
        })
    }

    fn dispatch_lexical_resolution(
        &self,
        request: &TextQueryRequest,
        budget: &RequestBudgetV1,
    ) -> SearchPlaneQueryIpcResponse {
        self.observed_route(
            QueryRoute::LexicalResolution,
            request.generation.as_ref(),
            || match self.plan_lexical_text_query(request, QueryRouteV1::Lexical, budget) {
                Ok(plan) => SearchPlaneQueryIpcResponse::ResolvedLexicalGeneration(plan.pin),
                Err(error) => SearchPlaneQueryIpcResponse::Error(core_error_to_ipc(error)),
            },
        )
    }

    fn dispatch_cluster_membership_batch_read(
        &self,
        request: &ClusterMembershipBatchReadRequestV1,
        budget: &RequestBudgetV1,
    ) -> SearchPlaneQueryIpcResponse {
        self.observed_route(
            QueryRoute::ClusterMembershipRead,
            Some(&request.generation),
            || match self.cluster_membership_batch_read(request, budget) {
                Ok(outcome) => SearchPlaneQueryIpcResponse::ClusterMembershipRead(outcome),
                Err(error) => SearchPlaneQueryIpcResponse::Error(core_error_to_ipc(error)),
            },
        )
    }

    /// Run one route under its intake, latency and outcome metrics
    /// (QI-BB-015).
    ///
    /// `requested_pin` is the generation the request named, if any; it
    /// labels these samples the way the intake and typed-error metrics
    /// already are labelled. The route's own pipeline metrics come from
    /// `run`; after it the wrapper adds one histogram
    /// (`lq_route_<route>_latency_ms`) and one counter
    /// (`lq_route_<route>_served_total`, or `_errors_total` when the answer
    /// is a typed error). A budget interruption adds one more counter
    /// naming which kind it was (`_deadline_exceeded_total` or
    /// `_cancelled_total`, QI-BB-002), so a timeout and a peer that left
    /// are never one number.
    fn observed_route(
        &self,
        route: QueryRoute,
        requested_pin: Option<&GenerationPin>,
        run: impl FnOnce() -> SearchPlaneQueryIpcResponse,
    ) -> SearchPlaneQueryIpcResponse {
        self.emit_intake_metric(requested_pin);
        let started = Instant::now();
        let response = run();
        let latency = elapsed_millis_metric(started.elapsed());
        self.emit_metric(
            requested_pin,
            &route.metric_name("latency_ms"),
            MetricKind::Histogram,
            latency,
        );
        let outcome = if let SearchPlaneQueryIpcResponse::Error(error) = &response {
            if let Some(suffix) = interruption_route_suffix(error.code) {
                self.emit_metric(
                    requested_pin,
                    &route.metric_name(suffix),
                    MetricKind::Counter,
                    1.0,
                );
            }
            "errors_total"
        } else {
            "served_total"
        };
        self.emit_metric(
            requested_pin,
            &route.metric_name(outcome),
            MetricKind::Counter,
            1.0,
        );
        response
    }

    /// `lq_route_<route>_examined_candidates_total` (QI-BB-015): the
    /// candidates the route materialized before cutting the page, from
    /// the window it answered with.
    fn emit_examined_candidates_metric(
        &self,
        route: QueryRoute,
        pin: &GenerationPin,
        window: &quanta_index_contract::QueryResultWindowV2,
    ) {
        self.emit_metric(
            Some(pin),
            &route.metric_name("examined_candidates_total"),
            MetricKind::Counter,
            examined_candidates_metric(window),
        );
    }

    fn dispatch_text(
        &self,
        request: &TextQueryRequest,
        budget: &RequestBudgetV1,
    ) -> SearchPlaneQueryIpcResponse {
        let requested_pin = request.generation.clone();
        self.observed_route(QueryRoute::Lexical, requested_pin.as_ref(), || {
            match self.lexical_with_execution(request, budget) {
                Ok((response, execution)) => {
                    self.emit_planner_metric(&response.generation);
                    self.emit_engine_activity_metrics(
                        &response.generation,
                        execution.executed_engines().len(),
                        execution.touched_engines().len(),
                    );
                    self.emit_examined_candidates_metric(
                        QueryRoute::Lexical,
                        &response.generation,
                        &response.window,
                    );
                    SearchPlaneQueryIpcResponse::Text(response)
                }
                Err(err) => {
                    self.emit_error_metric(requested_pin.as_ref(), &err);
                    SearchPlaneQueryIpcResponse::Error(core_error_to_ipc(err))
                }
            }
        })
    }

    fn dispatch_symbol(
        &self,
        request: SymbolQueryRequest,
        budget: &RequestBudgetV1,
    ) -> SearchPlaneQueryIpcResponse {
        let requested_pin = request.generation.clone();
        self.observed_route(QueryRoute::Symbol, requested_pin.as_ref(), || {
            match self.symbol_with_execution(request, budget) {
                Ok((response, execution)) => {
                    self.emit_planner_metric(&response.generation);
                    self.emit_engine_activity_metrics(
                        &response.generation,
                        execution.executed_engines().len(),
                        execution.touched_engines().len(),
                    );
                    self.emit_examined_candidates_metric(
                        QueryRoute::Symbol,
                        &response.generation,
                        &response.window,
                    );
                    SearchPlaneQueryIpcResponse::Symbol(response)
                }
                Err(err) => {
                    self.emit_error_metric(requested_pin.as_ref(), &err);
                    SearchPlaneQueryIpcResponse::Error(core_error_to_ipc(err))
                }
            }
        })
    }

    fn dispatch_semantic(
        &self,
        request: SemanticQueryRequest,
        budget: &RequestBudgetV1,
    ) -> SearchPlaneQueryIpcResponse {
        let requested_pin = request.generation.clone();
        self.observed_route(QueryRoute::Semantic, requested_pin.as_ref(), || match self
            .semantic_query(request, budget)
        {
            Ok(response) => {
                self.emit_planner_metric(&response.generation);
                self.emit_engine_activity_metrics(
                    &response.generation,
                    response.explanation.engines_executed.len(),
                    response.explanation.engines_touched.len(),
                );
                self.emit_early_stop_metric(
                    &response.generation,
                    response.explanation.early_stop_reason,
                );
                self.emit_examined_candidates_metric(
                    QueryRoute::Semantic,
                    &response.generation,
                    &response.window,
                );
                SearchPlaneQueryIpcResponse::Semantic(response)
            }
            Err(err) => {
                self.emit_error_metric(requested_pin.as_ref(), &err);
                SearchPlaneQueryIpcResponse::Error(core_error_to_ipc(err))
            }
        })
    }

    fn dispatch_semantic_work_bounded_v1(
        &self,
        request: SemanticWorkBoundedQueryRequestV1,
        budget: &RequestBudgetV1,
    ) -> SearchPlaneQueryIpcResponse {
        let requested_pin = request.query.generation.clone();
        self.observed_route(QueryRoute::Semantic, requested_pin.as_ref(), || {
            let execute = || -> Result<SemanticWorkBoundedQueryResponseV1, CoreError> {
                if request.max_work_units == 0 || request.max_work_units > quanta_index_contract::SEMANTIC_WORK_OPERATIONAL_CAP_V1 {
                    return Err(CoreError::InvalidContract(format!(
                        "bounded semantic work allowance must be within 1..={} units",
                        quanta_index_contract::SEMANTIC_WORK_OPERATIONAL_CAP_V1
                    )));
                }
                if request.query.generation.is_none()
                    || request.query.generation_selector.is_some()
                    || request.query.lexical_scope.is_some()
                    || !request.query.constraints.is_unconstrained()
                {
                    return Err(CoreError::InvalidContract(
                        "bounded semantic query requires an exact generation and no lexical scope or constraints".into(),
                    ));
                }
                let bounded_budget = budget
                    .clone()
                    .with_semantic_work_limit_v1(request.max_work_units)?;
                let response = self.semantic_query(request.query, &bounded_budget)?;
                let charged_work_units = bounded_budget
                    .semantic_work_consumed_v1()?
                    .ok_or_else(|| CoreError::InvalidContract(
                        "bounded semantic work ledger was lost".into(),
                    ))?;
                if charged_work_units == 0 || charged_work_units > request.max_work_units {
                    return Err(CoreError::InvalidContract(
                        "bounded semantic work settlement is invalid".into(),
                    ));
                }
                Ok(SemanticWorkBoundedQueryResponseV1 {
                    query: response,
                    charged_work_units,
                })
            };
            match execute() {
                Ok(response) => {
                    self.emit_planner_metric(&response.query.generation);
                    self.emit_engine_activity_metrics(
                        &response.query.generation,
                        response.query.explanation.engines_executed.len(),
                        response.query.explanation.engines_touched.len(),
                    );
                    self.emit_examined_candidates_metric(
                        QueryRoute::Semantic,
                        &response.query.generation,
                        &response.query.window,
                    );
                    SearchPlaneQueryIpcResponse::SemanticWorkBoundedV1(response)
                }
                Err(error) => {
                    self.emit_error_metric(requested_pin.as_ref(), &error);
                    SearchPlaneQueryIpcResponse::Error(core_error_to_ipc(error))
                }
            }
        })
    }

    fn dispatch_hybrid(
        &self,
        request: HybridQueryRequest,
        budget: &RequestBudgetV1,
    ) -> SearchPlaneQueryIpcResponse {
        let requested_pin = request.text_query.generation.clone();
        self.observed_route(QueryRoute::Hybrid, requested_pin.as_ref(), || {
            match self.hybrid_query(request, budget) {
                Ok(response) => {
                    self.emit_planner_metric(&response.generation);
                    self.emit_engine_activity_metrics(
                        &response.generation,
                        response.explanation.engines_executed.len(),
                        response.explanation.engines_touched.len(),
                    );
                    self.emit_merge_count_metric(&response.generation, response.results.len());
                    self.emit_early_stop_metric(
                        &response.generation,
                        response.explanation.early_stop_reason,
                    );
                    self.emit_examined_candidates_metric(
                        QueryRoute::Hybrid,
                        &response.generation,
                        &response.window,
                    );
                    SearchPlaneQueryIpcResponse::Hybrid(response)
                }
                Err(err) => {
                    self.emit_error_metric(requested_pin.as_ref(), &err);
                    SearchPlaneQueryIpcResponse::Error(core_error_to_ipc(err))
                }
            }
        })
    }

    fn dispatch_hybrid_seed(
        &self,
        request: &HybridSeedQueryRequest,
        budget: &RequestBudgetV1,
    ) -> SearchPlaneQueryIpcResponse {
        let requested_pin = request.text_query.generation.as_ref();
        self.observed_route(QueryRoute::HybridSeed, requested_pin, || {
            match self.hybrid_seed(request, budget) {
                Ok(response) => {
                    self.emit_planner_metric(&response.generation);
                    self.emit_engine_activity_metrics(
                        &response.generation,
                        response.explanation.engines_executed.len(),
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
                    self.emit_examined_candidates_metric(
                        QueryRoute::HybridSeed,
                        &response.generation,
                        &response.window,
                    );
                    SearchPlaneQueryIpcResponse::HybridSeed(response)
                }
                Err(err) => {
                    self.emit_error_metric(requested_pin, &err);
                    SearchPlaneQueryIpcResponse::Error(core_error_to_ipc(err))
                }
            }
        })
    }

    fn dispatch_history(
        &self,
        request: &HistoryQueryRequest,
        budget: &RequestBudgetV1,
    ) -> SearchPlaneQueryIpcResponse {
        let requested_pin = request.text_query.generation.as_ref();
        self.observed_route(QueryRoute::History, requested_pin, || {
            match self.history(request, budget) {
                Ok(response) => {
                    self.emit_planner_metric(&response.generation);
                    self.emit_engine_activity_metrics(
                        &response.generation,
                        1,
                        usize::from(!response.commits.is_empty() || !response.diffs.is_empty()),
                    );
                    self.emit_merge_count_metric(
                        &response.generation,
                        response.commits.len().saturating_add(response.diffs.len()),
                    );
                    // History reports what it examined under its order
                    // directly; that is the examined count, not the page.
                    self.emit_metric(
                        Some(&response.generation),
                        &QueryRoute::History.metric_name("examined_candidates_total"),
                        MetricKind::Counter,
                        quanta_index_core::count_as_f64(response.examined),
                    );
                    SearchPlaneQueryIpcResponse::History(response)
                }
                Err(err) => {
                    self.emit_error_metric(requested_pin, &err);
                    SearchPlaneQueryIpcResponse::Error(core_error_to_ipc(err))
                }
            }
        })
    }

    fn dispatch_structural(
        &self,
        request: &StructuralQueryRequest,
        budget: &RequestBudgetV1,
    ) -> SearchPlaneQueryIpcResponse {
        let requested_pin = request.text_query.generation.as_ref();
        self.observed_route(QueryRoute::Structural, requested_pin, || {
            match self.structural(request, budget) {
                Ok(response) => {
                    self.emit_planner_metric(&response.generation);
                    self.emit_engine_activity_metrics(
                        &response.generation,
                        1,
                        usize::from(!response.results.is_empty()),
                    );
                    self.emit_merge_count_metric(&response.generation, response.results.len());
                    self.emit_metric(
                        Some(&response.generation),
                        &QueryRoute::Structural.metric_name("examined_candidates_total"),
                        MetricKind::Counter,
                        quanta_index_core::count_as_f64(response.examined),
                    );
                    SearchPlaneQueryIpcResponse::Structural(response)
                }
                Err(err) => {
                    self.emit_error_metric(requested_pin, &err);
                    SearchPlaneQueryIpcResponse::Error(core_error_to_ipc(err))
                }
            }
        })
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
        self.observed_route(QueryRoute::RepoMap, Some(&requested_pin), || {
            match self.repo_map(request, budget) {
                Ok(response) => {
                    let response_pin = GenerationPin::new(
                        response.repo_id.clone(),
                        response.revision_id.clone(),
                        response.manifest_generation,
                    );
                    self.emit_planner_metric(&response_pin);
                    self.emit_engine_activity_metrics(
                        &response_pin,
                        1,
                        usize::from(!response.entries.is_empty()),
                    );
                    self.emit_merge_count_metric(&response_pin, response.entries.len());
                    SearchPlaneQueryIpcResponse::RepoMapQuery(response)
                }
                Err(err) => {
                    self.emit_error_metric(Some(&requested_pin), &err);
                    SearchPlaneQueryIpcResponse::Error(core_error_to_ipc(err))
                }
            }
        })
    }

    fn dispatch_explain(
        &self,
        request: SearchPlaneExplainQueryRequest,
        budget: &RequestBudgetV1,
    ) -> SearchPlaneQueryIpcResponse {
        let requested_pin = request.generation.clone();
        self.observed_route(QueryRoute::Explain, Some(&requested_pin), || {
            match self.explain_query(request, budget) {
                Ok(response) => {
                    self.emit_planner_metric(&response.generation);
                    self.emit_engine_activity_metrics(
                        &response.generation,
                        response.explanation.engines_executed.len(),
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
        })
    }

    /// The runtime-metadata route: the lexical text plan over the dirty
    /// overlay and structural chunk universe, read at one auxiliary epoch
    /// (`routes/runtime_metadata.rs`); refusals come back as the typed
    /// error the route raised.
    fn dispatch_runtime_metadata(
        &self,
        request: &RuntimeMetadataQueryRequest,
        budget: &RequestBudgetV1,
    ) -> SearchPlaneQueryIpcResponse {
        let requested_pin = request.text_query.generation.as_ref();
        self.observed_route(QueryRoute::RuntimeMetadata, requested_pin, || {
            match self.runtime_metadata(request, budget) {
                Ok(response) => {
                    self.emit_planner_metric(&response.generation);
                    self.emit_engine_activity_metrics(
                        &response.generation,
                        1,
                        usize::from(!response.results.is_empty()),
                    );
                    self.emit_merge_count_metric(&response.generation, response.results.len());
                    self.emit_metric(
                        Some(&response.generation),
                        &QueryRoute::RuntimeMetadata.metric_name("examined_candidates_total"),
                        MetricKind::Counter,
                        quanta_index_core::count_as_f64(response.examined),
                    );
                    SearchPlaneQueryIpcResponse::RuntimeMetadata(response)
                }
                Err(err) => {
                    self.emit_error_metric(requested_pin, &err);
                    SearchPlaneQueryIpcResponse::Error(core_error_to_ipc(err))
                }
            }
        })
    }

    pub(super) fn emit_metric(
        &self,
        pin: Option<&GenerationPin>,
        name: &str,
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

    /// Count invoked and post-filter contributing lanes separately. A
    /// zero-hit backend increments executed fanout but not contribution.
    fn emit_engine_activity_metrics(
        &self,
        pin: &GenerationPin,
        executed: usize,
        contributed: usize,
    ) {
        self.emit_metric(
            Some(pin),
            "lq_engine_fanout_count",
            MetricKind::Histogram,
            metric_count_value(executed),
        );
        self.emit_metric(
            Some(pin),
            "lq_lane_contribution_count",
            MetricKind::Histogram,
            metric_count_value(contributed),
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
