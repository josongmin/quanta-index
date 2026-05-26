//! Search-plane query orchestration using the in-memory readiness ledger as the
//! source of truth.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, RwLock};

use crate::{
    ActivationCatalog, ActiveGenerationRecord, Ledger, SEARCH_OWNED_SEMANTIC_DIMENSION,
    lower_lexical_text_query,
    lowering::{lower_sourcegraph_bridge_query_text, lower_sourcegraph_structural_query_text},
    query_embedder::{HashingQueryTextEmbedder, QueryTextEmbedderPort},
    readiness::{HistoryAuthorityState, RuntimeMetadataState, StructuralAuthorityState},
};
use quanta_index_contract::lex::{CommitSha, LexicalErrorCode};
use quanta_index_contract::{
    BridgeCandidate, BridgeQueryRequest, BridgeScope, ChunkRecord, CommitCandidate, DiffCandidate,
    EarlyStopReason, EngineTouched, GenerationPin, GenerationSelector, HistoryQueryRequest,
    HybridQueryRequest, HybridQueryResponse, LQ_VERSION_TAG, LexicalCandidate, LqCase, LqExpr,
    LqFilter, LqLeaf, LqOptions, LqQuery, LqSpan, LqStructuralBlock, LqStructuralConstraint,
    LqStructuralConstraintOperand, LqStructuralExpr, LqStructuralHoleRef, LqStructuralNode, LqType,
    LqYesNoOnly, ManifestGeneration, PlannerStage, PlannerTraceEntry, RepoId, RepoMapQueryRequest,
    RepoMapQueryResponse, RevisionId, RuntimeMetadataQueryRequest, SearchExplanation,
    SearchPlaneBridgeQueryResponse, SearchPlaneExplainQueryRequest,
    SearchPlaneExplainQueryResponse, SearchPlaneHistoryQueryResponse, SearchPlaneIpcError,
    SearchPlaneQueryIpcRequest, SearchPlaneQueryIpcResponse,
    SearchPlaneRuntimeMetadataQueryResponse, SearchPlaneStructuralQueryResponse,
    SearchPlaneTrackKind, SemanticQueryRequest, SemanticQueryResponse, StructuralQueryRequest,
    SymbolQueryRequest, SymbolQueryResponse, TextQueryRequest, TextQueryResponse, TextQuerySyntax,
};
use quanta_index_core::domains::structural::{
    StructuralExecutableFilter, StructuralProducerPort,
    StructuralQueryRequest as DomainStructuralQueryRequest,
};
use quanta_index_core::{
    CoreError, ExplainQueryPort, HybridOrchestratorPolicy, HybridQueryPort, LexicalIndexOpenPort,
    LexicalPolicy, LexicalQueryPort, RepoMapPolicy, RepoMapQueryPort, SemanticIndexOpenPort,
    SemanticPolicy, SemanticQueryPort, StructuralMatchBinding, StructuralMatchCandidate,
    StructuralService,
};
use quanta_index_lq_bridge::export_bridge_candidate_packet;
use quanta_index_lq_obs::{
    CardinalityGuard, Dimensions, MetricKind, MetricSample, OBS_OVERFLOW_LABEL, ObsError,
    validate_dimensions,
};

const ERR_INVALID: &str = "INVALID_REQUEST";
const ERR_NOT_READY: &str = "NOT_READY";
const ERR_NOT_FOUND: &str = "NOT_FOUND";
const ERR_NOT_IMPLEMENTED: &str = "NOT_IMPLEMENTED";
const ERR_INTERNAL: &str = "INTERNAL";
const ERR_HISTORY_PRODUCER_UNAVAILABLE: &str = "HISTORY_PRODUCER_UNAVAILABLE";
const ERR_HISTORY_GENERATION_NOT_READY: &str = "HISTORY_GENERATION_NOT_READY";
const ERR_HISTORY_SHARD_UNAVAILABLE: &str = "HISTORY_SHARD_UNAVAILABLE";

pub trait QueryObsSink {
    fn emit(&self, sample: MetricSample);
}

struct NoopQueryObsSink;

impl QueryObsSink for NoopQueryObsSink {
    fn emit(&self, _sample: MetricSample) {}
}

#[derive(Default)]
pub struct BoundedQueryObsStore {
    guard: Mutex<CardinalityGuard>,
    samples: Mutex<Vec<MetricSample>>,
    errors: Mutex<Vec<ObsError>>,
}

impl BoundedQueryObsStore {
    fn record_error(&self, err: ObsError) {
        let mut guard = lock_or_recover(&self.errors);
        guard.push(err);
    }

    #[must_use]
    pub fn snapshot(&self) -> Vec<MetricSample> {
        lock_or_recover(&self.samples).clone()
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
        samples.push(sample);
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
            if code.contains("PARSE")
                || code.contains("TRANSLATE_FAIL")
                || code.contains("INVALID_VECTOR")
                || code.contains("INVALID_REQUEST")
                || code.contains("HOLE_KIND_UNSUPPORTED") =>
        {
            "lq_typed_error_parse_total"
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
        CoreError::InvalidContract(_) => "lq_typed_error_invalid_total",
        CoreError::NotImplemented(_) | CoreError::NotFound(_) => "lq_typed_error_unavailable_total",
        CoreError::Typed { .. } => "lq_typed_error_other_total",
    }
}

pub struct SearchPlaneDispatcher {
    lex_opener: Arc<dyn LexicalIndexOpenPort + Send + Sync>,
    sem_opener: Arc<dyn SemanticIndexOpenPort + Send + Sync>,
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

#[derive(Clone, Debug)]
struct SemanticSelection {
    pin: GenerationPin,
    expected_manifest_digest: Option<String>,
}

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

    pub fn new_with_obs(
        lex_opener: Arc<dyn LexicalIndexOpenPort + Send + Sync>,
        sem_opener: Arc<dyn SemanticIndexOpenPort + Send + Sync>,
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
            repo_map_query,
            structural_producer,
            ledger,
            activation_catalog,
            query_embedder,
            obs_sink,
        }
    }

    /// Lower the request and forward it to the live lexical searcher.
    fn lexical(&self, request: &TextQueryRequest) -> Result<TextQueryResponse, CoreError> {
        let lowered = lower_lexical_text_query(request)?;
        LexicalPolicy::validate_query(&lowered)?;
        let pin = resolve_lexical_request_pin(
            self.activation_catalog.as_ref(),
            request,
            SearchPlaneTrackKind::Lexical,
            "lexical",
        )?;
        let materialized = self.snapshot_lex_materialized(&pin.repo_id, &pin.revision_id)?;
        LexicalPolicy::validate_query_against_readiness(pin.manifest_generation, materialized)?;
        let searcher =
            self.lex_opener
                .open(&pin.repo_id, &pin.revision_id, pin.manifest_generation)?;
        let results = searcher.search(&lowered, request.top_k)?;
        Ok(TextQueryResponse {
            generation: pin,
            results,
        })
    }

    fn symbol(&self, request: SymbolQueryRequest) -> Result<SymbolQueryResponse, CoreError> {
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
            generation: Some(pin.clone()),
            generation_selector: None,
            top_k: request.top_k,
        };
        let lowered = lower_lexical_text_query(&lexical_request)?;
        LexicalPolicy::validate_query(&lowered)?;
        let materialized = self.snapshot_lex_materialized(&pin.repo_id, &pin.revision_id)?;
        LexicalPolicy::validate_query_against_readiness(pin.manifest_generation, materialized)?;
        let searcher =
            self.lex_opener
                .open(&pin.repo_id, &pin.revision_id, pin.manifest_generation)?;
        let results = searcher.search_symbols(&lowered, request.top_k)?;
        Ok(SymbolQueryResponse {
            generation: pin,
            results,
        })
    }

    fn semantic(&self, request: &SemanticQueryRequest) -> Result<SemanticQueryResponse, CoreError> {
        SemanticPolicy::validate_top_k(request.top_k)?;
        let selection =
            resolve_semantic_request_selection(self.activation_catalog.as_ref(), request)?;
        let pin = selection.pin.clone();
        self.validate_semantic_selection(&selection, "semantic")?;
        let scope_candidate_ids = if let Some(scope) = request.lexical_scope.as_ref() {
            let lowered_scope = lower_lexical_text_query(scope)?;
            LexicalPolicy::validate_query(&lowered_scope)?;
            let lex_materialized =
                self.snapshot_lex_materialized(&pin.repo_id, &pin.revision_id)?;
            LexicalPolicy::validate_query_against_readiness(
                pin.manifest_generation,
                lex_materialized,
            )?;
            let searcher =
                self.lex_opener
                    .open(&pin.repo_id, &pin.revision_id, pin.manifest_generation)?;
            let scoped = searcher.search_all(&lowered_scope)?;
            Some(
                scoped
                    .into_iter()
                    .map(|candidate| candidate.candidate_id)
                    .collect::<BTreeSet<_>>(),
            )
        } else {
            None
        };
        let searcher =
            self.sem_opener
                .open(&pin.repo_id, &pin.revision_id, pin.manifest_generation)?;
        let query_vector = self
            .query_embedder
            .embed_query(request.query_text.as_str())
            .map_err(|err| prefix_semantic_query_error("semantic", err))?;
        let results = if let Some(scope_ids) = scope_candidate_ids.as_ref() {
            searcher.search_scoped(&query_vector, scope_ids, request.top_k)?
        } else {
            searcher.search(&query_vector, request.top_k)?
        };
        let early_stop_reason = scope_candidate_ids.as_ref().and_then(|scope_ids| {
            let limit = top_k_limit(request.top_k);
            if scope_ids.len() > results.len() && results.len() == limit {
                Some(EarlyStopReason::CountReached)
            } else {
                None
            }
        });
        let explanation = build_semantic_response_explanation(
            scope_candidate_ids.as_ref().map_or(0, BTreeSet::len),
            scope_candidate_ids.is_some(),
            results.len(),
            early_stop_reason,
        );
        Ok(SemanticQueryResponse {
            generation: pin,
            results,
            explanation,
        })
    }

    fn hybrid(&self, request: &HybridQueryRequest) -> Result<HybridQueryResponse, CoreError> {
        HybridOrchestratorPolicy::validate_top_k(request.top_k)?;
        let selection =
            resolve_hybrid_request_selection(self.activation_catalog.as_ref(), request)?;
        let pin = selection.pin.clone();
        let lex_materialized = self.snapshot_lex_materialized(&pin.repo_id, &pin.revision_id)?;
        LexicalPolicy::validate_query_against_readiness(pin.manifest_generation, lex_materialized)?;
        self.validate_semantic_selection(&selection, "hybrid")?;

        let lex_searcher =
            self.lex_opener
                .open(&pin.repo_id, &pin.revision_id, pin.manifest_generation)?;
        let sem_searcher =
            self.sem_opener
                .open(&pin.repo_id, &pin.revision_id, pin.manifest_generation)?;
        let lexical_query = lower_lexical_text_query(&request.text_query)?;
        LexicalPolicy::validate_query(&lexical_query)?;
        let internal_top_k = HybridOrchestratorPolicy::over_fetch_top_k(request.top_k);
        let mut lex_results = lex_searcher.search(&lexical_query, internal_top_k)?;
        stabilize_ranked_candidates(&mut lex_results);
        let lexical_ids = lex_results
            .iter()
            .map(|candidate| candidate.candidate_id.clone())
            .collect::<BTreeSet<_>>();
        let query_vector = self
            .query_embedder
            .embed_query(request.semantic_query_text.as_str())
            .map_err(|err| prefix_semantic_query_error("hybrid", err))?;
        let mut sem_results =
            sem_searcher.search_scoped(&query_vector, &lexical_ids, internal_top_k)?;
        stabilize_ranked_candidates(&mut sem_results);
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
        let fused = HybridOrchestratorPolicy::fuse_rrf(&lex_results, &sem_results, request.top_k);
        let early_stop_reason = if fused_universe_size > fused.len() {
            Some(EarlyStopReason::CountReached)
        } else {
            None
        };
        let explanation = build_hybrid_response_explanation(
            lexical_ids.len(),
            lex_results.len(),
            sem_results.len(),
            fused.len(),
            internal_top_k,
            early_stop_reason,
        );
        Ok(HybridQueryResponse {
            generation: pin,
            results: fused,
            explanation,
        })
    }

    fn runtime_metadata(
        &self,
        request: &RuntimeMetadataQueryRequest,
    ) -> Result<SearchPlaneRuntimeMetadataQueryResponse, CoreError> {
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
        let guard = self
            .ledger
            .read()
            .map_err(|_poisoned| CoreError::Storage("search-plane ledger poisoned".to_string()))?;
        let runtime_state = guard
            .runtime_state(&pin.repo_id, &pin.revision_id, pin.manifest_generation)
            .ok_or_else(|| {
                CoreError::NotReady(format!(
                    "runtime metadata: generation {} is not materialized",
                    pin.manifest_generation.get()
                ))
            })?;
        let structural_state = guard
            .structural_state(&pin.repo_id, &pin.revision_id, pin.manifest_generation)
            .ok_or_else(|| {
                CoreError::NotReady(format!(
                    "runtime metadata: lexical chunk authority for generation {} is not materialized",
                    pin.manifest_generation.get()
                ))
            })?;
        let results = execute_runtime_metadata_query(
            &pin,
            &lowered,
            runtime_state,
            structural_state,
            request.text_query.top_k,
        )?;
        drop(guard);
        Ok(SearchPlaneRuntimeMetadataQueryResponse {
            generation: pin,
            results,
        })
    }

    fn history(
        &self,
        request: &HistoryQueryRequest,
    ) -> Result<SearchPlaneHistoryQueryResponse, CoreError> {
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
        let guard = self
            .ledger
            .read()
            .map_err(|_poisoned| CoreError::Storage("search-plane ledger poisoned".to_string()))?;
        let history_state = resolve_history_state(&guard, &pin, &lowered)?;
        let (commits, diffs) =
            execute_history_query(&pin, &lowered, history_state, request.text_query.top_k)?;
        drop(guard);
        Ok(SearchPlaneHistoryQueryResponse {
            generation: pin,
            commits,
            diffs,
        })
    }

    fn structural(
        &self,
        request: &StructuralQueryRequest,
    ) -> Result<SearchPlaneStructuralQueryResponse, CoreError> {
        let (pin, lowered) =
            lower_structural_query_request(self.activation_catalog.as_ref(), request)?;
        let results = self.execute_structural_results(&pin, &lowered, request.text_query.top_k)?;
        Ok(SearchPlaneStructuralQueryResponse {
            generation: pin,
            results,
        })
    }

    fn bridge(
        &self,
        request: &BridgeQueryRequest,
    ) -> Result<SearchPlaneBridgeQueryResponse, CoreError> {
        let (scope, pin, candidates) = match lower_bridge_query_request(&request.text_query)? {
            BridgeExecutionPlan::Lexical(lowered) => {
                let pin = resolve_lexical_request_pin(
                    self.activation_catalog.as_ref(),
                    &request.text_query,
                    SearchPlaneTrackKind::Lexical,
                    "bridge",
                )?;
                let materialized =
                    self.snapshot_lex_materialized(&pin.repo_id, &pin.revision_id)?;
                LexicalPolicy::validate_query_against_readiness(
                    pin.manifest_generation,
                    materialized,
                )?;
                LexicalPolicy::validate_query(&lowered)?;
                let searcher = self.lex_opener.open(
                    &pin.repo_id,
                    &pin.revision_id,
                    pin.manifest_generation,
                )?;
                let candidates = searcher
                    .search(&lowered, request.text_query.top_k)?
                    .into_iter()
                    .map(BridgeCandidate::Lexical)
                    .collect();
                (BridgeScope::Lexical, pin, candidates)
            }
            BridgeExecutionPlan::Structural(lowered) => {
                let pin = resolve_lexical_request_pin(
                    self.activation_catalog.as_ref(),
                    &request.text_query,
                    SearchPlaneTrackKind::Structural,
                    "bridge",
                )?;
                let candidates = self
                    .execute_structural_results(&pin, &lowered, request.text_query.top_k)?
                    .into_iter()
                    .map(BridgeCandidate::Structural)
                    .collect();
                (BridgeScope::Structural, pin, candidates)
            }
        };
        let packet = export_bridge_candidate_packet(
            request.target,
            scope,
            &pin,
            &request.text_query,
            candidates,
        );
        Ok(SearchPlaneBridgeQueryResponse {
            generation: pin,
            packet,
        })
    }

    fn explain(
        &self,
        request: SearchPlaneExplainQueryRequest,
    ) -> Result<SearchPlaneExplainQueryResponse, CoreError> {
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
            self.lex_opener
                .open(&pin.repo_id, &pin.revision_id, pin.manifest_generation)?;
        let probe_text = if request.candidate.snippet.is_empty() {
            request.candidate.candidate_id.clone()
        } else {
            request.candidate.snippet.clone()
        };
        let probe = build_probe_query(&probe_text);
        let results = searcher.search(&probe, default_top_k())?;
        let present = results
            .iter()
            .any(|c| c.candidate_id == request.candidate.candidate_id);
        let summary = if present {
            format!(
                "candidate {} present in lexical index at generation {} (repo={}, revision={}, score={:.4}, snippet_len={})",
                request.candidate.candidate_id,
                pin.manifest_generation.get(),
                pin.repo_id.as_str(),
                pin.revision_id.as_str(),
                request.candidate.score,
                request.candidate.snippet.len()
            )
        } else {
            format!(
                "candidate {} NOT present in lexical index at generation {} (stale, removed, or never indexed)",
                request.candidate.candidate_id,
                pin.manifest_generation.get()
            )
        };
        Ok(SearchPlaneExplainQueryResponse {
            generation: pin,
            explanation: SearchExplanation {
                planner_trace: vec![
                    PlannerTraceEntry {
                        stage: PlannerStage::Plan,
                        detail: "explain-presence-probe".to_string(),
                    },
                    PlannerTraceEntry {
                        stage: PlannerStage::Merge,
                        detail: format!("candidate_present={present}"),
                    },
                ],
                engines_touched: vec![EngineTouched::Lexical],
                early_stop_reason: None,
                contributions: Vec::new(),
                ranker_weights_hash: [0u8; 32],
                strategy: "presence_probe".to_string(),
                summary,
            },
        })
    }

    fn repo_map(&self, request: RepoMapQueryRequest) -> Result<RepoMapQueryResponse, CoreError> {
        RepoMapPolicy::validate_query(&request)?;
        self.repo_map_query.query(request)
    }

    fn execute_structural_results(
        &self,
        pin: &GenerationPin,
        lowered: &LqQuery,
        top_k: u32,
    ) -> Result<Vec<quanta_index_contract::StructuralCandidate>, CoreError> {
        let service = StructuralService::new(Arc::clone(&self.structural_producer));
        let requested_lang = extract_structural_requested_lang(&lowered.expr)?;
        let (requested_lang, executable_filters) =
            extract_structural_filters(lowered, requested_lang.as_deref())?;
        let mut ctx = StructuralEvalContext::default();
        let candidates = evaluate_structural_expr(
            &mut ctx,
            &service,
            pin,
            &lowered.expr,
            requested_lang.as_deref(),
            &executable_filters,
            &lowered.options,
            None,
        )?;
        let mut results = project_structural_query_results(candidates);
        results.truncate(top_k_limit(top_k));
        Ok(results)
    }

    #[must_use]
    pub fn dispatch(&self, request: SearchPlaneQueryIpcRequest) -> SearchPlaneQueryIpcResponse {
        // One arm per variant; each delegates to a private handler that
        // returns the already-wrapped `SearchPlaneQueryIpcResponse`. Adding a
        // new variant means: add one handler fn + add one match arm — no
        // edits to encode/decode/match/factory all at once.
        match request {
            SearchPlaneQueryIpcRequest::Text(req) => self.dispatch_text(req),
            SearchPlaneQueryIpcRequest::Symbol(req) => self.dispatch_symbol(req),
            SearchPlaneQueryIpcRequest::Semantic(req) => self.dispatch_semantic(req),
            SearchPlaneQueryIpcRequest::Hybrid(req) => self.dispatch_hybrid(req),
            SearchPlaneQueryIpcRequest::History(req) => self.dispatch_history(&req),
            SearchPlaneQueryIpcRequest::Structural(req) => self.dispatch_structural(&req),
            SearchPlaneQueryIpcRequest::Bridge(req) => self.dispatch_bridge(&req),
            SearchPlaneQueryIpcRequest::RepoMapQuery(req) => self.dispatch_repo_map(req),
            SearchPlaneQueryIpcRequest::Explain(req) => self.dispatch_explain(req),
            SearchPlaneQueryIpcRequest::RuntimeMetadata(req) => {
                self.dispatch_runtime_metadata(&req)
            }
        }
    }

    fn dispatch_text(&self, request: TextQueryRequest) -> SearchPlaneQueryIpcResponse {
        let requested_pin = request.generation.clone();
        self.emit_intake_metric(requested_pin.as_ref());
        match self.lexical_query(request) {
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

    fn dispatch_symbol(&self, request: SymbolQueryRequest) -> SearchPlaneQueryIpcResponse {
        let requested_pin = request.generation.clone();
        self.emit_intake_metric(requested_pin.as_ref());
        match self.symbol(request) {
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

    fn dispatch_semantic(&self, request: SemanticQueryRequest) -> SearchPlaneQueryIpcResponse {
        let requested_pin = request.generation.clone();
        self.emit_intake_metric(requested_pin.as_ref());
        match self.semantic_query(request) {
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

    fn dispatch_hybrid(&self, request: HybridQueryRequest) -> SearchPlaneQueryIpcResponse {
        let requested_pin = request.text_query.generation.clone();
        self.emit_intake_metric(requested_pin.as_ref());
        match self.hybrid_query(request) {
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

    fn dispatch_history(&self, request: &HistoryQueryRequest) -> SearchPlaneQueryIpcResponse {
        let requested_pin = request.text_query.generation.as_ref();
        self.emit_intake_metric(requested_pin);
        match self.history(request) {
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

    fn dispatch_structural(&self, request: &StructuralQueryRequest) -> SearchPlaneQueryIpcResponse {
        let requested_pin = request.text_query.generation.as_ref();
        self.emit_intake_metric(requested_pin);
        match self.structural(request) {
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

    fn dispatch_bridge(&self, request: &BridgeQueryRequest) -> SearchPlaneQueryIpcResponse {
        let requested_pin = request.text_query.generation.as_ref();
        self.emit_intake_metric(requested_pin);
        match self.bridge(request) {
            Ok(response) => {
                self.emit_planner_metric(&response.generation);
                self.emit_engine_fanout_metric(&response.generation, 1);
                self.emit_merge_count_metric(
                    &response.generation,
                    response.packet.candidates.len(),
                );
                SearchPlaneQueryIpcResponse::Bridge(response)
            }
            Err(err) => {
                self.emit_error_metric(requested_pin, &err);
                SearchPlaneQueryIpcResponse::Error(core_error_to_ipc(err))
            }
        }
    }

    fn dispatch_repo_map(&self, request: RepoMapQueryRequest) -> SearchPlaneQueryIpcResponse {
        let requested_pin = GenerationPin::new(
            request.repo_id.clone(),
            request.revision_id.clone(),
            request.manifest_generation,
        );
        self.emit_intake_metric(Some(&requested_pin));
        match self.repo_map(request) {
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
    ) -> SearchPlaneQueryIpcResponse {
        let requested_pin = request.generation.clone();
        self.emit_intake_metric(Some(&requested_pin));
        match self.explain_query(request) {
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
    ) -> SearchPlaneQueryIpcResponse {
        let requested_pin = request.text_query.generation.as_ref();
        self.emit_intake_metric(requested_pin);
        match self.runtime_metadata(request) {
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
    fn lexical_query(&self, request: TextQueryRequest) -> Result<TextQueryResponse, CoreError> {
        self.lexical(&request)
    }
}

impl SemanticQueryPort for SearchPlaneDispatcher {
    fn semantic_query(
        &self,
        request: SemanticQueryRequest,
    ) -> Result<SemanticQueryResponse, CoreError> {
        self.semantic(&request)
    }
}

impl HybridQueryPort for SearchPlaneDispatcher {
    fn hybrid_query(&self, request: HybridQueryRequest) -> Result<HybridQueryResponse, CoreError> {
        self.hybrid(&request)
    }
}

impl ExplainQueryPort for SearchPlaneDispatcher {
    fn explain_query(
        &self,
        request: SearchPlaneExplainQueryRequest,
    ) -> Result<SearchPlaneExplainQueryResponse, CoreError> {
        self.explain(request)
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
    SearchPlaneIpcError { code, message }
}

const fn default_top_k() -> u32 {
    50
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct ExecutableTextPlaneValidationState {
    saw_dirty: bool,
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
                | LqFilter::Content { .. } => Ok(()),
                LqFilter::Repo { .. }
                | LqFilter::Lang { .. }
                | LqFilter::Select { .. }
                | LqFilter::Dirty { .. }
                | LqFilter::Fork { .. }
                | LqFilter::Archived { .. }
                | LqFilter::Visibility { .. }
                | LqFilter::Context { .. } => Err(CoreError::NotImplemented(
                    "history: one or more filters are not executable on the current adapter set"
                        .to_string(),
                )),
            },
            Self::RuntimeMetadata => match filter {
                LqFilter::Dirty { mode } => {
                    state.saw_dirty = true;
                    if matches!(mode, LqYesNoOnly::No) {
                        return Err(CoreError::NotImplemented(
                            "runtime metadata: dirty:no is not executable without a clean-document universe"
                                .to_string(),
                        ));
                    }
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
                if !state.saw_dirty {
                    return Err(CoreError::InvalidContract(
                        "runtime metadata: dirty:{yes|only} filter is required".to_string(),
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
    validate_executable_text_surface(&query.expr, policy.plane_name())?;
    for filter in &query.filters {
        if let LqFilter::Content { leaf } = filter {
            validate_leaf_surface(leaf, policy.plane_name())?;
        }
    }
    Ok(())
}

fn validate_history_query(query: &LqQuery) -> Result<(), CoreError> {
    validate_executable_text_query(query, ExecutableTextPlanePolicy::History)
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

fn resolve_history_state<'a>(
    ledger: &'a Ledger,
    pin: &GenerationPin,
    query: &LqQuery,
) -> Result<&'a HistoryAuthorityState, CoreError> {
    let Some(history_state) =
        ledger.history_state(&pin.repo_id, &pin.revision_id, pin.manifest_generation)
    else {
        let lexical_materialized = ledger.track_materialized(
            &pin.repo_id,
            &pin.revision_id,
            SearchPlaneTrackKind::Lexical,
        );
        return Err(history_absent_error(pin, lexical_materialized));
    };
    ensure_history_shards_ready(history_state, query)?;
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
    let requirements = history_shard_requirements(query);
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

fn history_shard_requirements(query: &LqQuery) -> HistoryShardRequirements {
    let mut requirements = match history_query_type(query) {
        Some(LqType::Commit) => HistoryShardRequirements {
            commits: true,
            ..HistoryShardRequirements::default()
        },
        Some(LqType::Diff) | None => HistoryShardRequirements {
            commits: true,
            diff_hunks: true,
            ..HistoryShardRequirements::default()
        },
        Some(LqType::File | LqType::Path | LqType::Symbol | LqType::Repo) => {
            HistoryShardRequirements::default()
        }
    };
    for filter in &query.filters {
        if let LqFilter::Rev { spec } = filter
            && CommitSha::from_hex(spec).is_err()
        {
            requirements.refs = true;
            requirements.tags = true;
        }
    }
    requirements
}

fn history_shard_unavailable(message: impl Into<String>) -> CoreError {
    CoreError::Typed {
        code: ERR_HISTORY_SHARD_UNAVAILABLE.to_string(),
        message: message.into(),
    }
}

fn validate_runtime_metadata_query(query: &LqQuery) -> Result<(), CoreError> {
    validate_executable_text_query(query, ExecutableTextPlanePolicy::RuntimeMetadata)
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

enum BridgeExecutionPlan {
    Lexical(LqQuery),
    Structural(LqQuery),
}

fn lower_bridge_query_request(
    request: &TextQueryRequest,
) -> Result<BridgeExecutionPlan, CoreError> {
    let lowered = match request.syntax {
        TextQuerySyntax::Native => lower_lexical_text_query(request)?,
        TextQuerySyntax::Sourcegraph => lower_sourcegraph_bridge_query_text(&request.query_text)?,
    };
    if extract_structural_requested_lang(&lowered.expr).is_ok() {
        validate_structural_feature_surface(&lowered)?;
        return Ok(BridgeExecutionPlan::Structural(lowered));
    }
    Ok(BridgeExecutionPlan::Lexical(lowered))
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

fn extract_structural_requested_lang(expr: &LqExpr) -> Result<Option<String>, CoreError> {
    let mut requested_lang = None;
    collect_structural_requested_lang(expr, &mut requested_lang)?;
    Ok(requested_lang)
}

fn collect_structural_requested_lang(
    expr: &LqExpr,
    requested_lang: &mut Option<String>,
) -> Result<(), CoreError> {
    match expr {
        LqExpr::Empty => Err(structural_invalid_request(
            "query must include at least one structural `match { ... }` leaf",
        )),
        LqExpr::Leaf(LqLeaf::StructuralBlock(block)) => {
            merge_structural_requested_lang(requested_lang, block.lang.as_deref())
        }
        LqExpr::Leaf(_) => Err(structural_invalid_request(
            "query must lower to a structural-only boolean tree of `match { ... }` leaves",
        )),
        LqExpr::Not(inner) => collect_structural_requested_lang(inner, requested_lang),
        LqExpr::All(children) | LqExpr::Any(children) => {
            if children.is_empty() {
                return Err(structural_invalid_request(
                    "query must include at least one structural `match { ... }` leaf",
                ));
            }
            for child in children {
                collect_structural_requested_lang(child, requested_lang)?;
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

fn evaluate_structural_expr(
    ctx: &mut StructuralEvalContext,
    service: &StructuralService,
    pin: &GenerationPin,
    expr: &LqExpr,
    requested_lang: Option<&str>,
    filters: &[StructuralExecutableFilter],
    options: &LqOptions,
    seed: Option<&StructuralCandidateBuckets>,
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
        LqExpr::Leaf(_) => Err(structural_invalid_request(
            "query must lower to a structural-only boolean tree of `match { ... }` leaves",
        )),
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
        let combined = merge_structural_match_sets(candidate_id, left_matches, right_matches);
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
            | LqFilter::Type { .. }
            | LqFilter::Select { .. }
            | LqFilter::Dirty { .. }
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
        LqFilter::Type { .. } => "type",
        LqFilter::Select { .. } => "select",
        LqFilter::Dirty { .. } => "dirty",
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

fn validate_executable_text_surface(expr: &LqExpr, plane: &str) -> Result<(), CoreError> {
    match expr {
        LqExpr::Empty => Ok(()),
        LqExpr::Leaf(leaf) => validate_leaf_surface(leaf, plane),
        LqExpr::Not(inner) => validate_executable_text_surface(inner, plane),
        LqExpr::All(children) | LqExpr::Any(children) => {
            for child in children {
                validate_executable_text_surface(child, plane)?;
            }
            Ok(())
        }
    }
}

fn validate_leaf_surface(leaf: &LqLeaf, plane: &str) -> Result<(), CoreError> {
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

fn execute_history_query(
    _pin: &GenerationPin,
    query: &LqQuery,
    state: &HistoryAuthorityState,
    top_k: u32,
) -> Result<(Vec<CommitCandidate>, Vec<DiffCandidate>), CoreError> {
    let include_commits = !matches!(history_query_type(query), Some(LqType::Diff));
    let include_diffs = !matches!(history_query_type(query), Some(LqType::Commit));
    let limit = top_k_limit(top_k);
    let mut commits = Vec::new();
    let mut diffs = Vec::new();

    if include_commits {
        for record in state.commits().values() {
            if history_commit_matches(query, state, record)? {
                commits.push(commit_candidate_from_record(record));
                if commits.len() >= limit {
                    break;
                }
            }
        }
    }

    if include_diffs {
        for (key, record) in state.diff_hunks() {
            let Some(commit) = state.commits().get(&key.commit_sha()) else {
                continue;
            };
            if history_diff_matches(query, state, key, record, commit)? {
                diffs.push(diff_candidate_from_record(key, record));
                if diffs.len() >= limit {
                    break;
                }
            }
        }
    }
    Ok((commits, diffs))
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
    for chunk_id in runtime_state.dirty_docs().keys() {
        let Some(chunk) = structural_state.chunks().get(chunk_id) else {
            continue;
        };
        if runtime_candidate_matches(query, chunk)? {
            out.push(lexical_candidate_from_chunk(pin, chunk));
            if out.len() >= limit {
                break;
            }
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
            LqFilter::Repo { .. }
            | LqFilter::Lang { .. }
            | LqFilter::Select { .. }
            | LqFilter::Dirty { .. }
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
            LqFilter::Repo { .. }
            | LqFilter::Lang { .. }
            | LqFilter::Select { .. }
            | LqFilter::Dirty { .. }
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

fn runtime_candidate_matches(query: &LqQuery, chunk: &ChunkRecord) -> Result<bool, CoreError> {
    for filter in &query.filters {
        match filter {
            LqFilter::Dirty { mode } => {
                if matches!(mode, LqYesNoOnly::No) {
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
    }
}

fn top_k_limit(top_k: u32) -> usize {
    usize::try_from(top_k).map_or(usize::MAX, core::convert::identity)
}

fn unix_seconds_from_ms(ms: u64) -> i64 {
    i64::try_from(ms.div_euclid(1_000)).map_or(i64::MAX, core::convert::identity)
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

fn resolve_semantic_request_selection(
    activation_catalog: &ActivationCatalog,
    request: &SemanticQueryRequest,
) -> Result<SemanticSelection, CoreError> {
    let outer_selection = match request.generation_selector.as_ref() {
        Some(selector) => Some(resolve_semantic_selector_selection(
            activation_catalog,
            selector,
            "semantic",
        )?),
        None => None,
    };
    let scope_pin = match request.lexical_scope.as_ref() {
        Some(scope) => Some(resolve_lexical_request_pin(
            activation_catalog,
            scope,
            SearchPlaneTrackKind::Lexical,
            "semantic scope",
        )?),
        None => None,
    };
    match (request.generation.clone(), outer_selection, scope_pin) {
        (Some(pin), Some(selection), Some(scope_pin))
            if pin != selection.pin || pin != scope_pin =>
        {
            Err(CoreError::InvalidContract(
                "semantic: scope generation does not match semantic request generation".to_string(),
            ))
        }
        (Some(pin), Some(selection), None) if pin != selection.pin => {
            Err(CoreError::InvalidContract(
                "semantic: explicit generation pin does not match generation selector resolution"
                    .to_string(),
            ))
        }
        (Some(pin), None, Some(scope_pin)) if pin != scope_pin => Err(CoreError::InvalidContract(
            "semantic: scope generation does not match semantic request generation".to_string(),
        )),
        (None, Some(selection), Some(scope_pin)) if selection.pin != scope_pin => {
            Err(CoreError::InvalidContract(
                "semantic: scope generation does not match semantic request generation".to_string(),
            ))
        }
        (Some(pin), Some(selection), _) => Ok(SemanticSelection {
            pin,
            expected_manifest_digest: selection.expected_manifest_digest,
        }),
        (Some(pin), None, _) | (None, None, Some(pin)) => Ok(SemanticSelection {
            pin,
            expected_manifest_digest: None,
        }),
        (None, Some(selection), _) => Ok(selection),
        (None, None, None) => Err(CoreError::InvalidContract(
            "semantic: generation pin required".to_string(),
        )),
    }
}

fn resolve_hybrid_request_selection(
    activation_catalog: &ActivationCatalog,
    request: &HybridQueryRequest,
) -> Result<SemanticSelection, CoreError> {
    let lexical_pin = resolve_lexical_request_pin(
        activation_catalog,
        &request.text_query,
        SearchPlaneTrackKind::Lexical,
        "hybrid text_query",
    )?;
    let semantic_selection = match request.generation_selector.as_ref() {
        Some(selector) => Some(resolve_semantic_selector_selection(
            activation_catalog,
            selector,
            "hybrid",
        )?),
        None => None,
    };
    match (request.generation.clone(), semantic_selection) {
        (Some(pin), Some(selection)) if pin != selection.pin || pin != lexical_pin => {
            Err(CoreError::InvalidContract(
                "hybrid: lexical generation does not match semantic generation".to_string(),
            ))
        }
        (Some(pin), None) if pin != lexical_pin => Err(CoreError::InvalidContract(
            "hybrid: lexical generation does not match semantic generation".to_string(),
        )),
        (None, Some(selection)) if selection.pin != lexical_pin => Err(CoreError::InvalidContract(
            "hybrid: lexical generation does not match semantic generation".to_string(),
        )),
        (Some(pin), Some(selection)) => Ok(SemanticSelection {
            pin,
            expected_manifest_digest: selection.expected_manifest_digest,
        }),
        (Some(pin), None) => Ok(SemanticSelection {
            pin,
            expected_manifest_digest: None,
        }),
        (None, Some(selection)) => Ok(selection),
        (None, None) => Ok(SemanticSelection {
            pin: lexical_pin,
            expected_manifest_digest: None,
        }),
    }
}

fn build_probe_query(probe_text: &str) -> LqQuery {
    LqQuery {
        lq_version: LQ_VERSION_TAG,
        expr: LqExpr::Leaf(LqLeaf::Phrase(probe_text.to_string())),
        filters: Vec::new(),
        directives: Vec::new(),
        options: LqOptions::defaults(),
        source_span: LqSpan::eof(u32::try_from(probe_text.len()).map_or(u32::MAX, |n| n)),
    }
}

fn build_semantic_response_explanation(
    scope_candidate_count: usize,
    scoped: bool,
    result_count: usize,
    early_stop_reason: Option<EarlyStopReason>,
) -> SearchExplanation {
    let mut planner_trace = vec![PlannerTraceEntry {
        stage: PlannerStage::Plan,
        detail: format!("semantic.scope={scoped}"),
    }];
    if scoped {
        planner_trace.push(PlannerTraceEntry {
            stage: PlannerStage::ExecFanout,
            detail: format!("semantic.scope.text_candidates={scope_candidate_count}"),
        });
    }
    planner_trace.push(PlannerTraceEntry {
        stage: PlannerStage::Merge,
        detail: format!("semantic.results={result_count}"),
    });
    let engines_touched = if scoped {
        vec![EngineTouched::Lexical, EngineTouched::Semantic]
    } else {
        vec![EngineTouched::Semantic]
    };
    let summary = if scoped {
        format!(
            "semantic scoped query returned {result_count} candidates from text scope of {scope_candidate_count}"
        )
    } else {
        format!("semantic query returned {result_count} candidates")
    };
    SearchExplanation {
        planner_trace,
        engines_touched,
        early_stop_reason,
        contributions: Vec::new(),
        ranker_weights_hash: [0u8; 32],
        strategy: if scoped {
            "semantic_scoped".to_string()
        } else {
            "semantic".to_string()
        },
        summary,
    }
}

fn build_hybrid_response_explanation(
    lexical_universe_size: usize,
    lexical_hits: usize,
    semantic_hits: usize,
    fused_hits: usize,
    internal_top_k: u32,
    early_stop_reason: Option<EarlyStopReason>,
) -> SearchExplanation {
    SearchExplanation {
        planner_trace: vec![
            PlannerTraceEntry {
                stage: PlannerStage::Plan,
                detail: format!("hybrid.internal_top_k={internal_top_k}"),
            },
            PlannerTraceEntry {
                stage: PlannerStage::ExecFanout,
                detail: format!(
                    "hybrid.lexical_universe={lexical_universe_size}; lexical_hits={lexical_hits}; semantic_hits={semantic_hits}"
                ),
            },
            PlannerTraceEntry {
                stage: PlannerStage::Merge,
                detail: format!("hybrid.fused_results={fused_hits}"),
            },
        ],
        engines_touched: vec![EngineTouched::Lexical, EngineTouched::Semantic],
        early_stop_reason,
        contributions: Vec::new(),
        ranker_weights_hash: [0u8; 32],
        strategy: "rrf".to_string(),
        summary: format!(
            "hybrid fused {lexical_hits} lexical and {semantic_hits} semantic candidates into {fused_hits} results"
        ),
    }
}

fn prefix_semantic_query_error(plane: &str, err: CoreError) -> CoreError {
    match err {
        CoreError::Typed { code, message } => CoreError::Typed {
            code,
            message: format!("{plane}: {message}"),
        },
        other @ (CoreError::InvalidContract(_)
        | CoreError::NotReady(_)
        | CoreError::NotImplemented(_)
        | CoreError::NotFound(_)
        | CoreError::Storage(_)) => other,
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
    use std::sync::{Arc, Mutex, RwLock};

    use super::{
        BoundedQueryObsStore, ERR_HISTORY_GENERATION_NOT_READY, ERR_HISTORY_PRODUCER_UNAVAILABLE,
        ERR_HISTORY_SHARD_UNAVAILABLE, ERR_NOT_IMPLEMENTED, FailClosedStructuralProducer,
        QueryObsSink, SearchPlaneDispatcher, classify_error_metric_name, make_pin,
    };
    use crate::{
        ActivationCatalog, HashingQueryTextEmbedder, Ledger, QueryTextEmbedderPort,
        SEARCH_OWNED_SEMANTIC_DIMENSION,
    };
    use quanta_index_contract::channel::LexicalChannelOp;
    use quanta_index_contract::lex::{CommitRecord, CommitSha, SymbolKindCode, SymbolKindFamily};
    use quanta_index_contract::{
        BridgeQueryRequest, GenerationPin, GenerationSelector, HistoryQueryRequest,
        HybridQueryRequest, LexicalCandidate, ManifestGeneration, RepoId, RepoMapDocType,
        RepoMapEntryDto, RepoMapExactnessSummary, RepoMapGraphCoverageClass,
        RepoMapItemIndexAvailability, RepoMapQueryRequest, RepoMapQueryResponse,
        RepoMapRedactionState, RepoMapSnapshotMeta, RepoRelativePath, RevisionId,
        RuntimeMetadataQueryRequest, SearchPlaneActivateGenerationRequest,
        SearchPlaneQueryIpcRequest, SearchPlaneQueryIpcResponse, SemanticQueryRequest,
        SymbolCandidate, TextQueryRequest, TextQuerySyntax, UpsertCommit,
    };
    use quanta_index_core::{
        CoreError, LexicalIndexOpenPort, LexicalSearcher, RepoMapQueryPort, SemanticIndexOpenPort,
        SemanticSearcher,
    };
    use quanta_index_lq_bridge::BridgeErrorCode;
    use tempfile::tempdir;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn encode_cbor<T: serde::Serialize>(
        value: &T,
    ) -> Result<Vec<u8>, ciborium::ser::Error<std::io::Error>> {
        let mut buf: Vec<u8> = Vec::new();
        ciborium::into_writer(value, &mut buf)?;
        Ok(buf)
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
                    included: true,
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
            | SearchPlaneQueryIpcResponse::History(_)
            | SearchPlaneQueryIpcResponse::Structural(_)
            | SearchPlaneQueryIpcResponse::Bridge(_)
            | SearchPlaneQueryIpcResponse::Explain(_)
            | SearchPlaneQueryIpcResponse::Error(_)
            | quanta_index_contract::SearchPlaneQueryIpcResponse::RuntimeMetadata(_)) => {
                Err(format!("expected repo-map query response, got {other:?}").into())
            }
        }
    }

    fn test_activation_catalog() -> Result<Arc<ActivationCatalog>, Box<dyn std::error::Error>> {
        let dir = tempdir()?;
        Ok(Arc::new(ActivationCatalog::open(dir.keep())?))
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

    fn history_commit_sha() -> CommitSha {
        CommitSha::from_bytes([
            0x10, 0x32, 0x54, 0x76, 0x98, 0xba, 0xdc, 0xfe, 0x10, 0x32, 0x54, 0x76, 0x98, 0xba,
            0xdc, 0xfe, 0x10, 0x32, 0x54, 0x76,
        ])
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
            committer: "alice".to_string().into_boxed_str(),
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
                generation: Some(ready_pin()),
                generation_selector: None,
                top_k: 5,
            },
        })
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
        }
    }

    #[expect(
        clippy::panic,
        reason = "test fixture uses a canonical symbol kind literal"
    )]
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
            symbol_kind: match SymbolKindCode::new("function") {
                Ok(symbol_kind) => symbol_kind,
                Err(err) => panic!("test symbol kind is canonical: {err}"),
            },
            symbol_kind_family: Some(SymbolKindFamily::Callable),
        }
    }

    #[derive(Default)]
    struct RecordingSemanticState {
        search_vectors: Vec<Vec<f32>>,
        scoped_vectors: Vec<Vec<f32>>,
    }

    struct RecordingSemanticSearcher {
        state: Arc<Mutex<RecordingSemanticState>>,
    }

    impl SemanticSearcher for RecordingSemanticSearcher {
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
    }

    struct RecordingSemanticOpener {
        state: Arc<Mutex<RecordingSemanticState>>,
    }

    impl SemanticIndexOpenPort for RecordingSemanticOpener {
        fn open(
            &self,
            _repo: &RepoId,
            _revision: &RevisionId,
            _generation: ManifestGeneration,
        ) -> Result<Box<dyn SemanticSearcher>, CoreError> {
            Ok(Box::new(RecordingSemanticSearcher {
                state: Arc::clone(&self.state),
            }))
        }
    }

    struct StubLexicalSearcher {
        results: Vec<LexicalCandidate>,
    }

    impl LexicalSearcher for StubLexicalSearcher {
        fn search(
            &self,
            _query: &quanta_index_contract::LqQuery,
            _top_k: u32,
        ) -> Result<Vec<LexicalCandidate>, CoreError> {
            Ok(self.results.clone())
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
        search_all_calls: u32,
    }

    struct RecordingLexicalSearcher {
        state: Arc<Mutex<RecordingLexicalState>>,
        results: Vec<LexicalCandidate>,
    }

    impl LexicalSearcher for RecordingLexicalSearcher {
        fn search(
            &self,
            _query: &quanta_index_contract::LqQuery,
            top_k: u32,
        ) -> Result<Vec<LexicalCandidate>, CoreError> {
            self.state
                .lock()
                .map_err(|err| CoreError::Storage(format!("lexical state poisoned: {err}")))?
                .search_top_ks
                .push(top_k);
            Ok(self.results.clone())
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

        fn search_all(
            &self,
            _query: &quanta_index_contract::LqQuery,
        ) -> Result<Vec<LexicalCandidate>, CoreError> {
            let mut guard = self
                .state
                .lock()
                .map_err(|err| CoreError::Storage(format!("lexical state poisoned: {err}")))?;
            guard.search_all_calls = guard.search_all_calls.saturating_add(1);
            drop(guard);
            Ok(self.results.clone())
        }
    }

    struct RecordingLexicalOpener {
        state: Arc<Mutex<RecordingLexicalState>>,
        results: Vec<LexicalCandidate>,
    }

    impl LexicalIndexOpenPort for RecordingLexicalOpener {
        fn open(
            &self,
            _repo: &RepoId,
            _revision: &RevisionId,
            _generation: ManifestGeneration,
        ) -> Result<Box<dyn LexicalSearcher>, CoreError> {
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

        let response = into_repo_map_query_response(
            dispatcher.dispatch(SearchPlaneQueryIpcRequest::RepoMapQuery(repo_map_request())),
        )?;

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

        match dispatcher.dispatch(SearchPlaneQueryIpcRequest::Text(TextQueryRequest {
            syntax: TextQuerySyntax::Native,
            query_text: "needle".to_string(),
            generation: Some(make_pin(
                RepoId::new("repo-map-ipc"),
                RevisionId::new("rev-map-ipc"),
                ManifestGeneration::new(9),
            )),
            generation_selector: None,
            top_k: 50,
        })) {
            SearchPlaneQueryIpcResponse::Error(err) => {
                if err.code != "NOT_READY" {
                    return Err(format!("unexpected error code: {}", err.code).into());
                }
            }
            other @ (SearchPlaneQueryIpcResponse::Text(_)
            | SearchPlaneQueryIpcResponse::Symbol(_)
            | SearchPlaneQueryIpcResponse::Semantic(_)
            | SearchPlaneQueryIpcResponse::Hybrid(_)
            | SearchPlaneQueryIpcResponse::History(_)
            | SearchPlaneQueryIpcResponse::Structural(_)
            | SearchPlaneQueryIpcResponse::Bridge(_)
            | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
            | SearchPlaneQueryIpcResponse::Explain(_)
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
        let response = dispatcher.dispatch(SearchPlaneQueryIpcRequest::Text(TextQueryRequest {
            syntax: TextQuerySyntax::Sourcegraph,
            query_text: "repo:repo-map-ipc alpha".into(),
            generation: Some(pin.clone()),
            generation_selector: None,
            top_k: 2,
        }));

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
            | SearchPlaneQueryIpcResponse::History(_)
            | SearchPlaneQueryIpcResponse::Structural(_)
            | SearchPlaneQueryIpcResponse::Bridge(_)
            | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
            | SearchPlaneQueryIpcResponse::Explain(_)
            | SearchPlaneQueryIpcResponse::Error(_)
            | SearchPlaneQueryIpcResponse::RuntimeMetadata(_)) => {
                return Err(format!("expected Text response, got {other:?}").into());
            }
        }
        let guard = state
            .lock()
            .map_err(|err| format!("lexical state poisoned: {err}"))?;
        if guard.search_top_ks.as_slice() != [2] {
            return Err(format!(
                "expected sourcegraph route to forward top_k=2, got {:?}",
                guard.search_top_ks
            )
            .into());
        }
        drop(guard);
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

        let response = dispatcher.dispatch(SearchPlaneQueryIpcRequest::Text(TextQueryRequest {
            syntax: TextQuerySyntax::Sourcegraph,
            query_text: r#"patterntype:structural "function_item""#.into(),
            generation: Some(ready_pin()),
            generation_selector: None,
            top_k: 2,
        }));

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
    fn bridge_dispatch_forwards_text_query_top_k() -> TestResult {
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
        let response =
            dispatcher.dispatch(SearchPlaneQueryIpcRequest::Bridge(BridgeQueryRequest {
                text_query: TextQueryRequest {
                    syntax: TextQuerySyntax::Sourcegraph,
                    query_text: "alpha".to_string(),
                    generation: Some(pin),
                    generation_selector: None,
                    top_k: 7,
                },
                target: quanta_index_contract::BridgeTarget::CodeQl,
            }));

        match response {
            SearchPlaneQueryIpcResponse::Bridge(bridge) => {
                if bridge.packet.candidates.len() != 2 {
                    return Err(format!(
                        "expected 2 bridge candidates, got {}",
                        bridge.packet.candidates.len()
                    )
                    .into());
                }
            }
            other @ (SearchPlaneQueryIpcResponse::Text(_)
            | SearchPlaneQueryIpcResponse::Symbol(_)
            | SearchPlaneQueryIpcResponse::Semantic(_)
            | SearchPlaneQueryIpcResponse::Hybrid(_)
            | SearchPlaneQueryIpcResponse::History(_)
            | SearchPlaneQueryIpcResponse::Structural(_)
            | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
            | SearchPlaneQueryIpcResponse::Explain(_)
            | SearchPlaneQueryIpcResponse::Error(_)
            | quanta_index_contract::SearchPlaneQueryIpcResponse::RuntimeMetadata(_)) => {
                return Err(format!("expected Bridge response, got {other:?}").into());
            }
        }

        let guard = state
            .lock()
            .map_err(|err| format!("lexical state poisoned: {err}"))?;
        if guard.search_top_ks.as_slice() != [7] {
            return Err(format!(
                "expected bridge route to forward top_k=7, got {:?}",
                guard.search_top_ks
            )
            .into());
        }
        drop(guard);
        Ok(())
    }

    #[test]
    fn bridge_dispatch_routes_native_structural_query_to_structural_scope() -> TestResult {
        let lexical_state = Arc::new(Mutex::new(RecordingLexicalState::default()));
        let producer = Arc::new(PatternRoutingStructuralProducer::new());
        let dispatcher = SearchPlaneDispatcher::new(
            Arc::new(RecordingLexicalOpener {
                state: Arc::clone(&lexical_state),
                results: vec![candidate("lexical-should-not-run", 1.0)],
            }),
            Arc::new(RejectSemanticOpener),
            Arc::new(StubRepoMapQueryPort),
            producer,
            ready_ledger(),
            test_activation_catalog()?,
        );

        let response =
            dispatcher.dispatch(SearchPlaneQueryIpcRequest::Bridge(BridgeQueryRequest {
                text_query: TextQueryRequest {
                    syntax: TextQuerySyntax::Native,
                    query_text: "match { alpha } AND match { beta }".to_string(),
                    generation: Some(ready_pin()),
                    generation_selector: None,
                    top_k: 10,
                },
                target: quanta_index_contract::BridgeTarget::CodeQl,
            }));

        match response {
            SearchPlaneQueryIpcResponse::Bridge(bridge) => {
                if bridge.packet.scope != quanta_index_contract::BridgeScope::Structural {
                    return Err(format!(
                        "expected structural bridge scope, got {:?}",
                        bridge.packet.scope
                    )
                    .into());
                }
                let ids = bridge
                    .packet
                    .candidates
                    .iter()
                    .map(|candidate| match candidate {
                        quanta_index_contract::BridgeCandidate::Structural(candidate) => {
                            Ok(candidate.candidate_id.clone())
                        }
                        quanta_index_contract::BridgeCandidate::Lexical(candidate) => Err(format!(
                            "expected structural bridge candidate, got lexical `{}`",
                            candidate.candidate_id
                        )),
                    })
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(Box::<dyn std::error::Error>::from)?;
                if ids != vec!["chunk-shared".to_string()] {
                    return Err(
                        format!("unexpected structural bridge candidate ids: {ids:?}").into(),
                    );
                }
            }
            other @ (SearchPlaneQueryIpcResponse::Text(_)
            | SearchPlaneQueryIpcResponse::Symbol(_)
            | SearchPlaneQueryIpcResponse::Semantic(_)
            | SearchPlaneQueryIpcResponse::Hybrid(_)
            | SearchPlaneQueryIpcResponse::History(_)
            | SearchPlaneQueryIpcResponse::Structural(_)
            | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
            | SearchPlaneQueryIpcResponse::Explain(_)
            | SearchPlaneQueryIpcResponse::Error(_)
            | quanta_index_contract::SearchPlaneQueryIpcResponse::RuntimeMetadata(_)) => {
                return Err(format!("expected Bridge response, got {other:?}").into());
            }
        }

        if !lexical_state
            .lock()
            .map_err(|err| format!("lexical state poisoned: {err}"))?
            .search_top_ks
            .is_empty()
        {
            return Err("lexical opener must not execute for structural bridge route".into());
        }
        Ok(())
    }

    #[test]
    fn bridge_dispatch_routes_sourcegraph_structural_query_to_structural_scope() -> TestResult {
        let lexical_state = Arc::new(Mutex::new(RecordingLexicalState::default()));
        let producer = Arc::new(PatternRoutingStructuralProducer::new());
        let dispatcher = SearchPlaneDispatcher::new(
            Arc::new(RecordingLexicalOpener {
                state: Arc::clone(&lexical_state),
                results: vec![candidate("lexical-should-not-run", 1.0)],
            }),
            Arc::new(RejectSemanticOpener),
            Arc::new(StubRepoMapQueryPort),
            producer,
            ready_ledger(),
            test_activation_catalog()?,
        );

        let query_text =
            r#"patterntype:structural "alpha" OR patterntype:structural "beta""#.to_string();
        let response =
            dispatcher.dispatch(SearchPlaneQueryIpcRequest::Bridge(BridgeQueryRequest {
                text_query: TextQueryRequest {
                    syntax: TextQuerySyntax::Sourcegraph,
                    query_text: query_text.clone(),
                    generation: Some(ready_pin()),
                    generation_selector: None,
                    top_k: 10,
                },
                target: quanta_index_contract::BridgeTarget::CodeQl,
            }));

        match response {
            SearchPlaneQueryIpcResponse::Bridge(bridge) => {
                if bridge.packet.scope != quanta_index_contract::BridgeScope::Structural {
                    return Err(format!(
                        "expected structural bridge scope, got {:?}",
                        bridge.packet.scope
                    )
                    .into());
                }
                if bridge.packet.source_syntax.as_deref() != Some(query_text.as_str()) {
                    return Err(format!(
                        "unexpected structural bridge source_syntax: {:?}",
                        bridge.packet.source_syntax
                    )
                    .into());
                }
                if bridge.packet.translator_version.as_deref()
                    != Some(quanta_index_lq_bridge::TRANSLATOR_VERSION)
                {
                    return Err(format!(
                        "unexpected structural bridge translator_version: {:?}",
                        bridge.packet.translator_version
                    )
                    .into());
                }
                let ids = bridge
                    .packet
                    .candidates
                    .iter()
                    .map(|candidate| match candidate {
                        quanta_index_contract::BridgeCandidate::Structural(candidate) => {
                            Ok(candidate.candidate_id.clone())
                        }
                        quanta_index_contract::BridgeCandidate::Lexical(candidate) => Err(format!(
                            "expected structural bridge candidate, got lexical `{}`",
                            candidate.candidate_id
                        )),
                    })
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(Box::<dyn std::error::Error>::from)?;
                if ids != vec!["chunk-a".to_string(), "chunk-shared".to_string()] {
                    return Err(
                        format!("unexpected SG structural bridge candidate ids: {ids:?}").into(),
                    );
                }
            }
            other @ (SearchPlaneQueryIpcResponse::Text(_)
            | SearchPlaneQueryIpcResponse::Symbol(_)
            | SearchPlaneQueryIpcResponse::Semantic(_)
            | SearchPlaneQueryIpcResponse::Hybrid(_)
            | SearchPlaneQueryIpcResponse::History(_)
            | SearchPlaneQueryIpcResponse::Structural(_)
            | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
            | SearchPlaneQueryIpcResponse::Explain(_)
            | SearchPlaneQueryIpcResponse::Error(_)
            | quanta_index_contract::SearchPlaneQueryIpcResponse::RuntimeMetadata(_)) => {
                return Err(format!("expected Bridge response, got {other:?}").into());
            }
        }

        if !lexical_state
            .lock()
            .map_err(|err| format!("lexical state poisoned: {err}"))?
            .search_top_ks
            .is_empty()
        {
            return Err("lexical opener must not execute for SG structural bridge route".into());
        }
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

        let response =
            dispatcher.dispatch(SearchPlaneQueryIpcRequest::Semantic(SemanticQueryRequest {
                query_text: "focus alpha".to_string(),
                generation: Some(GenerationPin::new(
                    RepoId::new("repo-map-ipc"),
                    RevisionId::new("rev-map-ipc"),
                    ManifestGeneration::new(9),
                )),
                generation_selector: None,
                lexical_scope: None,
                top_k: 3,
            }));

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
            | SearchPlaneQueryIpcResponse::History(_)
            | SearchPlaneQueryIpcResponse::Structural(_)
            | SearchPlaneQueryIpcResponse::Bridge(_)
            | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
            | SearchPlaneQueryIpcResponse::Explain(_)
            | SearchPlaneQueryIpcResponse::Error(_)
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

    #[test]
    fn hybrid_dispatch_embeds_semantic_query_text() -> TestResult {
        let state = Arc::new(Mutex::new(RecordingSemanticState::default()));
        let dispatcher = SearchPlaneDispatcher::new(
            Arc::new(StubLexicalOpener {
                results: vec![candidate("alpha", 1.0), candidate("beta", 0.9)],
            }),
            Arc::new(RecordingSemanticOpener {
                state: Arc::clone(&state),
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
        let response =
            dispatcher.dispatch(SearchPlaneQueryIpcRequest::Hybrid(HybridQueryRequest {
                text_query: TextQueryRequest {
                    syntax: TextQuerySyntax::Sourcegraph,
                    query_text: "scope".to_string(),
                    generation: Some(pin.clone()),
                    generation_selector: None,
                    top_k: 2,
                },
                semantic_query_text: "scope alpha".to_string(),
                generation: Some(pin),
                generation_selector: None,
                top_k: 2,
            }));

        match response {
            SearchPlaneQueryIpcResponse::Hybrid(hybrid) => {
                if hybrid.results.is_empty() {
                    return Err("expected non-empty hybrid results".into());
                }
            }
            other @ (SearchPlaneQueryIpcResponse::Text(_)
            | SearchPlaneQueryIpcResponse::Symbol(_)
            | SearchPlaneQueryIpcResponse::Semantic(_)
            | SearchPlaneQueryIpcResponse::History(_)
            | SearchPlaneQueryIpcResponse::Structural(_)
            | SearchPlaneQueryIpcResponse::Bridge(_)
            | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
            | SearchPlaneQueryIpcResponse::Explain(_)
            | SearchPlaneQueryIpcResponse::Error(_)
            | quanta_index_contract::SearchPlaneQueryIpcResponse::RuntimeMetadata(_)) => {
                return Err(format!("expected Hybrid response, got {other:?}").into());
            }
        }

        let (scoped_vectors, search_vectors) = {
            let guard = state
                .lock()
                .map_err(|err| format!("semantic state poisoned: {err}"))?;
            (guard.scoped_vectors.clone(), guard.search_vectors.clone())
        };
        let expected = default_query_embedder().embed_query("scope alpha")?;
        if scoped_vectors.as_slice() != [expected] {
            return Err(format!("unexpected scoped vectors: {scoped_vectors:?}").into());
        }
        if !search_vectors.is_empty() {
            return Err(format!("unexpected global vectors: {search_vectors:?}").into());
        }
        Ok(())
    }

    #[test]
    fn semantic_dispatch_rejects_active_digest_mismatch_with_exact_code() -> TestResult {
        let dir = tempdir()?;
        let activation_catalog = Arc::new(ActivationCatalog::open(dir.keep())?);
        activation_catalog.activate(&SearchPlaneActivateGenerationRequest {
            repo_id: RepoId::new("repo-map-ipc"),
            revision_id: RevisionId::new("rev-map-ipc"),
            manifest_generation: ManifestGeneration::new(9),
            manifest_digest: "activation-digest-9".to_string(),
            tracks: vec![SearchPlaneTrackKind::Semantic],
        })?;
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

        let response =
            dispatcher.dispatch(SearchPlaneQueryIpcRequest::Semantic(SemanticQueryRequest {
                query_text: "focus alpha".to_string(),
                generation: None,
                generation_selector: Some(GenerationSelector::Active {
                    repo_id: RepoId::new("repo-map-ipc"),
                    revision_id: RevisionId::new("rev-map-ipc"),
                }),
                lexical_scope: None,
                top_k: 3,
            }));

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

        let response =
            dispatcher.dispatch(SearchPlaneQueryIpcRequest::Semantic(SemanticQueryRequest {
                query_text: "focus alpha".to_string(),
                generation: Some(GenerationPin::new(
                    RepoId::new("repo-map-ipc"),
                    RevisionId::new("rev-map-ipc"),
                    ManifestGeneration::new(9),
                )),
                generation_selector: None,
                lexical_scope: None,
                top_k: 3,
            }));

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

        let response =
            dispatcher.dispatch(SearchPlaneQueryIpcRequest::Hybrid(HybridQueryRequest {
                text_query: TextQueryRequest {
                    syntax: TextQuerySyntax::Native,
                    query_text: "scope".to_string(),
                    generation: Some(ready_pin()),
                    generation_selector: None,
                    top_k: 2,
                },
                semantic_query_text: "scope alpha".to_string(),
                generation: Some(ready_pin()),
                generation_selector: None,
                top_k: 2,
            }));

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
            self.readiness
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
        Ok(SearchPlaneDispatcher::new(
            Arc::new(RejectLexicalOpener),
            Arc::new(RejectSemanticOpener),
            Arc::new(StubRepoMapQueryPort),
            producer,
            ready_ledger(),
            test_activation_catalog()?,
        ))
    }

    fn dispatcher_with_obs(
        lex_opener: Arc<dyn LexicalIndexOpenPort + Send + Sync>,
        sem_opener: Arc<dyn SemanticIndexOpenPort + Send + Sync>,
        obs_sink: Arc<dyn QueryObsSink + Send + Sync>,
    ) -> Result<SearchPlaneDispatcher, Box<dyn std::error::Error>> {
        Ok(SearchPlaneDispatcher::new_with_obs(
            lex_opener,
            sem_opener,
            Arc::new(StubRepoMapQueryPort),
            Arc::new(FailClosedStructuralProducer),
            ready_ledger(),
            test_activation_catalog()?,
            default_query_embedder(),
            obs_sink,
        ))
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
            | SearchPlaneQueryIpcResponse::History(_)
            | SearchPlaneQueryIpcResponse::Structural(_)
            | SearchPlaneQueryIpcResponse::Bridge(_)
            | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
            | SearchPlaneQueryIpcResponse::Explain(_)
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

        let response =
            dispatcher.dispatch(SearchPlaneQueryIpcRequest::Hybrid(HybridQueryRequest {
                text_query: TextQueryRequest {
                    syntax: TextQuerySyntax::Native,
                    query_text: "scope".to_string(),
                    generation: Some(ready_pin()),
                    generation_selector: None,
                    top_k: 1,
                },
                semantic_query_text: "scope alpha".to_string(),
                generation: Some(ready_pin()),
                generation_selector: None,
                top_k: 1,
            }));
        match response {
            SearchPlaneQueryIpcResponse::Hybrid(_) => {}
            other @ (SearchPlaneQueryIpcResponse::Text(_)
            | SearchPlaneQueryIpcResponse::Symbol(_)
            | SearchPlaneQueryIpcResponse::Semantic(_)
            | SearchPlaneQueryIpcResponse::History(_)
            | SearchPlaneQueryIpcResponse::RuntimeMetadata(_)
            | SearchPlaneQueryIpcResponse::Structural(_)
            | SearchPlaneQueryIpcResponse::Bridge(_)
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
        let expected = vec![
            "lq_query_intake_total".to_string(),
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
    fn text_dispatch_parse_error_emits_closed_obs_metric() -> TestResult {
        let obs_sink = Arc::new(BoundedQueryObsStore::default());
        let dispatcher = dispatcher_with_obs(
            Arc::new(RejectLexicalOpener),
            Arc::new(RejectSemanticOpener),
            obs_sink.clone(),
        )?;

        let response = dispatcher.dispatch(SearchPlaneQueryIpcRequest::Text(TextQueryRequest {
            syntax: TextQuerySyntax::Native,
            query_text: "/(?<=needle_)x/".to_string(),
            generation: Some(ready_pin()),
            generation_selector: None,
            top_k: 10,
        }));
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

    #[test]
    fn repo_map_dispatch_emits_closed_obs_metrics() -> TestResult {
        let obs_sink = Arc::new(BoundedQueryObsStore::default());
        let dispatcher = SearchPlaneDispatcher::new_with_obs(
            Arc::new(RejectLexicalOpener),
            Arc::new(RejectSemanticOpener),
            Arc::new(StubRepoMapQueryPort),
            Arc::new(FailClosedStructuralProducer),
            Arc::new(RwLock::new(Ledger::default())),
            test_activation_catalog()?,
            default_query_embedder(),
            obs_sink.clone(),
        );

        let response =
            dispatcher.dispatch(SearchPlaneQueryIpcRequest::RepoMapQuery(repo_map_request()));
        match response {
            SearchPlaneQueryIpcResponse::RepoMapQuery(_) => {}
            other @ (SearchPlaneQueryIpcResponse::Text(_)
            | SearchPlaneQueryIpcResponse::Symbol(_)
            | SearchPlaneQueryIpcResponse::Semantic(_)
            | SearchPlaneQueryIpcResponse::Hybrid(_)
            | SearchPlaneQueryIpcResponse::History(_)
            | SearchPlaneQueryIpcResponse::RuntimeMetadata(_)
            | SearchPlaneQueryIpcResponse::Structural(_)
            | SearchPlaneQueryIpcResponse::Bridge(_)
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
    fn runtime_metadata_dispatch_unavailable_emits_closed_obs_metric() -> TestResult {
        let obs_sink = Arc::new(BoundedQueryObsStore::default());
        let dispatcher = dispatcher_with_obs(
            Arc::new(RejectLexicalOpener),
            Arc::new(RejectSemanticOpener),
            obs_sink.clone(),
        )?;

        let response = dispatcher.dispatch(SearchPlaneQueryIpcRequest::RuntimeMetadata(
            RuntimeMetadataQueryRequest {
                text_query: TextQueryRequest {
                    syntax: TextQuerySyntax::Native,
                    query_text: "dirty:no runtime".to_string(),
                    generation: Some(ready_pin()),
                    generation_selector: None,
                    top_k: 5,
                },
            },
        ));
        let (code, _message) =
            ipc_error_from(response).map_err(Box::<dyn std::error::Error>::from)?;
        if code != ERR_NOT_IMPLEMENTED {
            return Err(format!("expected {ERR_NOT_IMPLEMENTED}, got {code}").into());
        }
        let names = obs_sink
            .snapshot()
            .into_iter()
            .map(|sample| sample.name.into_string())
            .collect::<Vec<_>>();
        let expected = vec![
            "lq_query_intake_total".to_string(),
            "lq_typed_error_unavailable_total".to_string(),
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
    fn history_dispatch_success_emits_closed_obs_metrics() -> TestResult {
        let obs_sink = Arc::new(BoundedQueryObsStore::default());
        let commit_payload = encode_cbor(&history_commit_record())?;
        let dispatcher = SearchPlaneDispatcher::new_with_obs(
            Arc::new(RejectLexicalOpener),
            Arc::new(RejectSemanticOpener),
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

        let response = dispatcher.dispatch(history_query_request("type:commit fix"));
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
            | SearchPlaneQueryIpcResponse::RuntimeMetadata(_)
            | SearchPlaneQueryIpcResponse::Structural(_)
            | SearchPlaneQueryIpcResponse::Bridge(_)
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
            Arc::new(StubRepoMapQueryPort),
            Arc::new(FailClosedStructuralProducer),
            ready_ledger(),
            test_activation_catalog()?,
            default_query_embedder(),
            obs_sink.clone(),
        );

        let response = dispatcher.dispatch(history_query_request("type:commit fix"));
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
            Arc::new(StubRepoMapQueryPort),
            Arc::new(RecordingStructuralProducer::ready_with(vec![
                structural_match_candidate("chunk-tree"),
            ])),
            ready_ledger(),
            test_activation_catalog()?,
            default_query_embedder(),
            obs_sink.clone(),
        );

        let response = dispatcher.dispatch(SearchPlaneQueryIpcRequest::Structural(
            quanta_index_contract::StructuralQueryRequest {
                text_query: TextQueryRequest {
                    syntax: TextQuerySyntax::Native,
                    query_text: "match { :[x] }".to_string(),
                    generation: Some(ready_pin()),
                    generation_selector: None,
                    top_k: 4,
                },
            },
        ));
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
            | SearchPlaneQueryIpcResponse::History(_)
            | SearchPlaneQueryIpcResponse::RuntimeMetadata(_)
            | SearchPlaneQueryIpcResponse::Bridge(_)
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
    fn bridge_dispatch_structural_success_emits_closed_obs_metrics() -> TestResult {
        let obs_sink = Arc::new(BoundedQueryObsStore::default());
        let dispatcher = SearchPlaneDispatcher::new_with_obs(
            Arc::new(RejectLexicalOpener),
            Arc::new(RejectSemanticOpener),
            Arc::new(StubRepoMapQueryPort),
            Arc::new(PatternRoutingStructuralProducer::new()),
            ready_ledger(),
            test_activation_catalog()?,
            default_query_embedder(),
            obs_sink.clone(),
        );

        let response =
            dispatcher.dispatch(SearchPlaneQueryIpcRequest::Bridge(BridgeQueryRequest {
                text_query: TextQueryRequest {
                    syntax: TextQuerySyntax::Sourcegraph,
                    query_text: r#"patterntype:structural "alpha""#.to_string(),
                    generation: Some(ready_pin()),
                    generation_selector: None,
                    top_k: 10,
                },
                target: quanta_index_contract::BridgeTarget::CodeQl,
            }));
        match response {
            SearchPlaneQueryIpcResponse::Bridge(bridge) => {
                if bridge.generation != ready_pin()
                    || bridge.packet.scope != quanta_index_contract::BridgeScope::Structural
                    || bridge.packet.candidates.len() != 2
                {
                    return Err(format!("unexpected structural bridge response: {bridge:?}").into());
                }
            }
            other @ (SearchPlaneQueryIpcResponse::Text(_)
            | SearchPlaneQueryIpcResponse::Symbol(_)
            | SearchPlaneQueryIpcResponse::Semantic(_)
            | SearchPlaneQueryIpcResponse::Hybrid(_)
            | SearchPlaneQueryIpcResponse::History(_)
            | SearchPlaneQueryIpcResponse::RuntimeMetadata(_)
            | SearchPlaneQueryIpcResponse::Structural(_)
            | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
            | SearchPlaneQueryIpcResponse::Explain(_)
            | SearchPlaneQueryIpcResponse::Error(_)) => {
                return Err(format!("expected Bridge response, got {other:?}").into());
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
    fn bridge_dispatch_translate_fail_emits_parse_obs_metric() -> TestResult {
        let obs_sink = Arc::new(BoundedQueryObsStore::default());
        let dispatcher = SearchPlaneDispatcher::new_with_obs(
            Arc::new(RejectLexicalOpener),
            Arc::new(RejectSemanticOpener),
            Arc::new(StubRepoMapQueryPort),
            Arc::new(FailClosedStructuralProducer),
            ready_ledger(),
            test_activation_catalog()?,
            default_query_embedder(),
            obs_sink.clone(),
        );

        let response =
            dispatcher.dispatch(SearchPlaneQueryIpcRequest::Bridge(BridgeQueryRequest {
                text_query: TextQueryRequest {
                    syntax: TextQuerySyntax::Sourcegraph,
                    query_text: r#"patterntype:regexp "needle""#.to_string(),
                    generation: Some(ready_pin()),
                    generation_selector: None,
                    top_k: 10,
                },
                target: quanta_index_contract::BridgeTarget::CodeQl,
            }));
        let (code, _message) =
            ipc_error_from(response).map_err(Box::<dyn std::error::Error>::from)?;
        if code != BridgeErrorCode::BridgeTranslateFail.as_code_str() {
            return Err(format!(
                "expected {}, got {code}",
                BridgeErrorCode::BridgeTranslateFail.as_code_str()
            )
            .into());
        }

        assert_closed_obs_metrics(
            &obs_sink,
            &["lq_query_intake_total", "lq_typed_error_parse_total"],
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
                "lq_typed_error_invalid_total",
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

        let response = dispatcher.dispatch(SearchPlaneQueryIpcRequest::Text(TextQueryRequest {
            syntax: TextQuerySyntax::Native,
            query_text: "fork:only foo".to_string(),
            generation: Some(ready_pin()),
            generation_selector: None,
            top_k: 5,
        }));

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
            | SearchPlaneQueryIpcResponse::History(_)
            | SearchPlaneQueryIpcResponse::RuntimeMetadata(_)
            | SearchPlaneQueryIpcResponse::Structural(_)
            | SearchPlaneQueryIpcResponse::Bridge(_)
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
        if search_top_ks.as_slice() != [5] {
            return Err(format!(
                "expected metadata filter query to reach searcher with top_k=5, got {search_top_ks:?}"
            )
            .into());
        }
        Ok(())
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

        let response = dispatcher.dispatch(SearchPlaneQueryIpcRequest::Text(TextQueryRequest {
            syntax: TextQuerySyntax::Native,
            query_text: "rev:deadbeef foo".to_string(),
            generation: Some(ready_pin()),
            generation_selector: None,
            top_k: 5,
        }));

        let (code, _message) =
            ipc_error_from(response).map_err(Box::<dyn std::error::Error>::from)?;
        if code != "NOT_IMPLEMENTED" {
            return Err(format!("expected NOT_IMPLEMENTED, got {code}").into());
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
        let response = dispatcher.dispatch(SearchPlaneQueryIpcRequest::Text(TextQueryRequest {
            syntax: TextQuerySyntax::Native,
            query_text: "needle".to_string(),
            generation: Some(ready_pin()),
            generation_selector: None,
            top_k: 5,
        }));

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
            | SearchPlaneQueryIpcResponse::History(_)
            | SearchPlaneQueryIpcResponse::Structural(_)
            | SearchPlaneQueryIpcResponse::Bridge(_)
            | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
            | SearchPlaneQueryIpcResponse::Explain(_)
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
        if search_top_ks.as_slice() != [5] {
            return Err(format!(
                "expected searcher.search invoked with top_k=5, got {search_top_ks:?}"
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

        let response = dispatcher.dispatch(SearchPlaneQueryIpcRequest::Symbol(
            quanta_index_contract::SymbolQueryRequest {
                syntax: TextQuerySyntax::Native,
                query_text: "type:symbol MySymbol".to_string(),
                generation: Some(ready_pin()),
                generation_selector: None,
                top_k: 3,
            },
        ));

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
            | SearchPlaneQueryIpcResponse::History(_)
            | SearchPlaneQueryIpcResponse::RuntimeMetadata(_)
            | SearchPlaneQueryIpcResponse::Structural(_)
            | SearchPlaneQueryIpcResponse::Bridge(_)
            | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
            | SearchPlaneQueryIpcResponse::Explain(_)
            | SearchPlaneQueryIpcResponse::Error(_)) => {
                return Err(format!("expected Symbol response, got {other:?}").into());
            }
        }

        let guard = state
            .lock()
            .map_err(|err| format!("lexical state poisoned: {err}"))?;
        if guard.symbol_top_ks.as_slice() != [3] {
            return Err(format!(
                "expected symbol route to forward top_k=3, got {:?}",
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

        let response = dispatcher.dispatch(history_query_request("type:commit fix"));

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

        let response = dispatcher.dispatch(history_query_request("type:commit fix"));

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

        let response = dispatcher.dispatch(history_query_request("type:diff history"));

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

        let response = dispatcher.dispatch(SearchPlaneQueryIpcRequest::Structural(
            quanta_index_contract::StructuralQueryRequest {
                text_query: TextQueryRequest {
                    syntax: TextQuerySyntax::Native,
                    query_text: "match { :[x] }".to_string(),
                    generation: Some(ready_pin()),
                    generation_selector: None,
                    top_k: 4,
                },
            },
        ));

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
            | SearchPlaneQueryIpcResponse::History(_)
            | SearchPlaneQueryIpcResponse::RuntimeMetadata(_)
            | SearchPlaneQueryIpcResponse::Bridge(_)
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

        let response = dispatcher.dispatch(SearchPlaneQueryIpcRequest::Structural(
            quanta_index_contract::StructuralQueryRequest {
                text_query: TextQueryRequest {
                    syntax: TextQuerySyntax::Native,
                    query_text: "match { :[x] }".to_string(),
                    generation: Some(ready_pin()),
                    generation_selector: None,
                    top_k: 4,
                },
            },
        ));

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

        let response = dispatcher.dispatch(SearchPlaneQueryIpcRequest::Structural(
            quanta_index_contract::StructuralQueryRequest {
                text_query: TextQueryRequest {
                    syntax: TextQuerySyntax::Native,
                    query_text: "match { :[x] }".to_string(),
                    generation: Some(ready_pin()),
                    generation_selector: None,
                    top_k: 4,
                },
            },
        ));

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

        let response = dispatcher.dispatch(SearchPlaneQueryIpcRequest::Structural(
            quanta_index_contract::StructuralQueryRequest {
                text_query: TextQueryRequest {
                    syntax: TextQuerySyntax::Native,
                    query_text: "lang:java match { :[x] }".to_string(),
                    generation: Some(ready_pin()),
                    generation_selector: None,
                    top_k: 4,
                },
            },
        ));

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

        let response = dispatcher.dispatch(SearchPlaneQueryIpcRequest::Structural(
            quanta_index_contract::StructuralQueryRequest {
                text_query: TextQueryRequest {
                    syntax: TextQuerySyntax::Native,
                    query_text: "repo:repo-map-ipc file:src/lib.rs match { :[x] }".to_string(),
                    generation: Some(ready_pin()),
                    generation_selector: None,
                    top_k: 4,
                },
            },
        ));

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
            | SearchPlaneQueryIpcResponse::History(_)
            | SearchPlaneQueryIpcResponse::RuntimeMetadata(_)
            | SearchPlaneQueryIpcResponse::Bridge(_)
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

        let response = dispatcher.dispatch(SearchPlaneQueryIpcRequest::Structural(
            quanta_index_contract::StructuralQueryRequest {
                text_query: TextQueryRequest {
                    syntax: TextQuerySyntax::Native,
                    query_text: "select:repo match { :[x] }".to_string(),
                    generation: Some(ready_pin()),
                    generation_selector: None,
                    top_k: 4,
                },
            },
        ));

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
                    generation: Some(ready_pin()),
                    generation_selector: None,
                    top_k: 4,
                },
            },
        ));

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
            | SearchPlaneQueryIpcResponse::History(_)
            | SearchPlaneQueryIpcResponse::RuntimeMetadata(_)
            | SearchPlaneQueryIpcResponse::Bridge(_)
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

        let response = dispatcher.dispatch(SearchPlaneQueryIpcRequest::Structural(
            quanta_index_contract::StructuralQueryRequest {
                text_query: TextQueryRequest {
                    syntax: TextQuerySyntax::Native,
                    query_text: "match { function_item { { :[name.lambda] } } }".to_string(),
                    generation: Some(ready_pin()),
                    generation_selector: None,
                    top_k: 4,
                },
            },
        ));

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
    fn bridge_dispatch_rejects_typed_hole_kind_before_structural_execution() -> TestResult {
        let lexical_state = Arc::new(Mutex::new(RecordingLexicalState::default()));
        let producer = Arc::new(PatternRoutingStructuralProducer::new());
        let dispatcher = SearchPlaneDispatcher::new(
            Arc::new(RecordingLexicalOpener {
                state: Arc::clone(&lexical_state),
                results: vec![candidate("lexical-should-not-run", 1.0)],
            }),
            Arc::new(RejectSemanticOpener),
            Arc::new(StubRepoMapQueryPort),
            producer,
            ready_ledger(),
            test_activation_catalog()?,
        );

        let response =
            dispatcher.dispatch(SearchPlaneQueryIpcRequest::Bridge(BridgeQueryRequest {
                text_query: TextQueryRequest {
                    syntax: TextQuerySyntax::Native,
                    query_text: "match { function_item { { :[name.lambda] } } }".to_string(),
                    generation: Some(ready_pin()),
                    generation_selector: None,
                    top_k: 10,
                },
                target: quanta_index_contract::BridgeTarget::CodeQl,
            }));

        let (code, message) =
            ipc_error_from(response).map_err(Box::<dyn std::error::Error>::from)?;
        if code != "STR_HOLE_KIND_UNSUPPORTED" {
            return Err(format!("expected STR_HOLE_KIND_UNSUPPORTED, got {code}").into());
        }
        if !message.contains("typed hole kind `lambda`") {
            return Err(
                format!("expected typed-hole bridge rejection message, got {message}").into(),
            );
        }
        if !lexical_state
            .lock()
            .map_err(|err| format!("lexical state poisoned: {err}"))?
            .search_top_ks
            .is_empty()
        {
            return Err("lexical opener must not execute for typed-hole bridge rejection".into());
        }
        Ok(())
    }

    #[test]
    fn structural_dispatch_executes_structural_boolean_and_with_canonical_projection() -> TestResult
    {
        let producer = Arc::new(PatternRoutingStructuralProducer::new());
        let dispatcher = structural_dispatcher_with_producer(Arc::clone(&producer))?;

        let response = dispatcher.dispatch(SearchPlaneQueryIpcRequest::Structural(
            quanta_index_contract::StructuralQueryRequest {
                text_query: TextQueryRequest {
                    syntax: TextQuerySyntax::Native,
                    query_text: "match { alpha } AND match { beta }".to_string(),
                    generation: Some(ready_pin()),
                    generation_selector: None,
                    top_k: 10,
                },
            },
        ));

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
            | SearchPlaneQueryIpcResponse::History(_)
            | SearchPlaneQueryIpcResponse::RuntimeMetadata(_)
            | SearchPlaneQueryIpcResponse::Bridge(_)
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

        let response = dispatcher.dispatch(SearchPlaneQueryIpcRequest::Structural(
            quanta_index_contract::StructuralQueryRequest {
                text_query: TextQueryRequest {
                    syntax: TextQuerySyntax::Native,
                    query_text: "match { alpha } OR match { beta }".to_string(),
                    generation: Some(ready_pin()),
                    generation_selector: None,
                    top_k: 10,
                },
            },
        ));

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
            | SearchPlaneQueryIpcResponse::History(_)
            | SearchPlaneQueryIpcResponse::RuntimeMetadata(_)
            | SearchPlaneQueryIpcResponse::Bridge(_)
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

        let response = dispatcher.dispatch(SearchPlaneQueryIpcRequest::Structural(
            quanta_index_contract::StructuralQueryRequest {
                text_query: TextQueryRequest {
                    syntax: TextQuerySyntax::Native,
                    query_text: "match { alpha } AND NOT match { gamma }".to_string(),
                    generation: Some(ready_pin()),
                    generation_selector: None,
                    top_k: 10,
                },
            },
        ));

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
            | SearchPlaneQueryIpcResponse::History(_)
            | SearchPlaneQueryIpcResponse::RuntimeMetadata(_)
            | SearchPlaneQueryIpcResponse::Bridge(_)
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

        let response = dispatcher.dispatch(SearchPlaneQueryIpcRequest::Structural(
            quanta_index_contract::StructuralQueryRequest {
                text_query: TextQueryRequest {
                    syntax: TextQuerySyntax::Native,
                    query_text: "match { alpha } OR match { alpha }".to_string(),
                    generation: Some(ready_pin()),
                    generation_selector: None,
                    top_k: 10,
                },
            },
        ));

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
            | SearchPlaneQueryIpcResponse::History(_)
            | SearchPlaneQueryIpcResponse::RuntimeMetadata(_)
            | SearchPlaneQueryIpcResponse::Bridge(_)
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
    fn structural_dispatch_rejects_mixed_lexical_and_structural_boolean_before_execution()
    -> TestResult {
        let producer = Arc::new(PatternRoutingStructuralProducer::new());
        let dispatcher = structural_dispatcher_with_producer(Arc::clone(&producer))?;

        let response = dispatcher.dispatch(SearchPlaneQueryIpcRequest::Structural(
            quanta_index_contract::StructuralQueryRequest {
                text_query: TextQueryRequest {
                    syntax: TextQuerySyntax::Native,
                    query_text: "needle AND match { alpha }".to_string(),
                    generation: Some(ready_pin()),
                    generation_selector: None,
                    top_k: 10,
                },
            },
        ));

        let (code, message) =
            ipc_error_from(response).map_err(Box::<dyn std::error::Error>::from)?;
        if code != "STR_INVALID_REQUEST" {
            return Err(format!("expected STR_INVALID_REQUEST, got {code}").into());
        }
        if !message.contains("structural-only boolean tree") {
            return Err(format!("unexpected mixed-tree message: {message}").into());
        }
        if producer.readiness_calls.load(Ordering::SeqCst) != 0
            || producer.execute_calls.load(Ordering::SeqCst) != 0
        {
            return Err(
                "mixed lexical/structural boolean must fail before producer execution".into(),
            );
        }
        Ok(())
    }

    #[test]
    fn structural_dispatch_rejects_pure_negative_boolean_before_execution() -> TestResult {
        let producer = Arc::new(PatternRoutingStructuralProducer::new());
        let dispatcher = structural_dispatcher_with_producer(Arc::clone(&producer))?;

        let response = dispatcher.dispatch(SearchPlaneQueryIpcRequest::Structural(
            quanta_index_contract::StructuralQueryRequest {
                text_query: TextQueryRequest {
                    syntax: TextQuerySyntax::Native,
                    query_text: "NOT match { alpha }".to_string(),
                    generation: Some(ready_pin()),
                    generation_selector: None,
                    top_k: 10,
                },
            },
        ));

        let (code, message) =
            ipc_error_from(response).map_err(Box::<dyn std::error::Error>::from)?;
        if code != "STR_INVALID_REQUEST" {
            return Err(format!("expected STR_INVALID_REQUEST, got {code}").into());
        }
        if !message.contains("pure-negative structural boolean queries are not executable") {
            return Err(format!("unexpected pure-negative message: {message}").into());
        }
        if producer.readiness_calls.load(Ordering::SeqCst) != 0
            || producer.execute_calls.load(Ordering::SeqCst) != 0
        {
            return Err(
                "pure-negative structural boolean must fail before producer execution".into(),
            );
        }
        Ok(())
    }
}
