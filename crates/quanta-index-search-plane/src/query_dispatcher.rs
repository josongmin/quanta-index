//! Search-plane query orchestration using the in-memory readiness ledger as the
//! source of truth.

use std::collections::BTreeSet;
use std::sync::{Arc, RwLock};

use crate::{
    ActivationCatalog, Ledger, lower_lexical_text_query,
    lowering::lower_sourcegraph_structural_query_text,
    readiness::{HistoryAuthorityState, RuntimeMetadataState, StructuralAuthorityState},
};
use quanta_index_contract::lex::{CommitSha, LexicalErrorCode};
use quanta_index_contract::{
    BridgeQueryRequest, BridgeScope, ChunkRecord, CommitCandidate, DiffCandidate, EarlyStopReason,
    EngineTouched, GenerationPin, GenerationSelector, HistoryQueryRequest, HybridQueryRequest,
    HybridQueryResponse, LQ_VERSION_TAG, LqCase, LqExpr, LqFilter, LqLeaf, LqOptions, LqQuery,
    LqSpan, LqStructuralBlock, LqType, LqYesNoOnly, ManifestGeneration, PlannerStage,
    PlannerTraceEntry, RepoId, RepoMapQueryRequest, RepoMapQueryResponse, RevisionId,
    RuntimeMetadataQueryRequest, SearchExplanation, SearchPlaneBridgeQueryResponse,
    SearchPlaneExplainQueryRequest, SearchPlaneExplainQueryResponse,
    SearchPlaneHistoryQueryResponse, SearchPlaneIpcError, SearchPlaneQueryIpcRequest,
    SearchPlaneQueryIpcResponse, SearchPlaneRuntimeMetadataQueryResponse,
    SearchPlaneStructuralQueryResponse, SearchPlaneTrackKind, SemanticQueryRequest,
    SemanticQueryResponse, SemanticVectorRef, StructuralQueryRequest, SymbolQueryRequest,
    SymbolQueryResponse, TextQueryRequest, TextQueryResponse, TextQuerySyntax,
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

const ERR_INVALID: &str = "INVALID_REQUEST";
const ERR_NOT_READY: &str = "NOT_READY";
const ERR_NOT_FOUND: &str = "NOT_FOUND";
const ERR_NOT_IMPLEMENTED: &str = "NOT_IMPLEMENTED";
const ERR_INTERNAL: &str = "INTERNAL";
const ERR_HISTORY_PRODUCER_UNAVAILABLE: &str = "HISTORY_PRODUCER_UNAVAILABLE";
const ERR_HISTORY_GENERATION_NOT_READY: &str = "HISTORY_GENERATION_NOT_READY";
const ERR_HISTORY_SHARD_UNAVAILABLE: &str = "HISTORY_SHARD_UNAVAILABLE";

pub struct SearchPlaneDispatcher {
    lex_opener: Arc<dyn LexicalIndexOpenPort + Send + Sync>,
    sem_opener: Arc<dyn SemanticIndexOpenPort + Send + Sync>,
    repo_map_query: Arc<dyn RepoMapQueryPort + Send + Sync>,
    /// Structural producer adapter wired by the composition root.
    structural_producer: Arc<dyn StructuralProducerPort + Send + Sync>,
    ledger: Arc<RwLock<Ledger>>,
    activation_catalog: Arc<ActivationCatalog>,
}

pub type SearchPlaneQueryService = SearchPlaneDispatcher;
pub type SearchPlaneQueryDispatcher = SearchPlaneDispatcher;

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
        Self {
            lex_opener,
            sem_opener,
            repo_map_query,
            structural_producer,
            ledger,
            activation_catalog,
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
        let pin = resolve_semantic_request_pin(self.activation_catalog.as_ref(), request)?;
        let materialized = self.snapshot_sem_materialized(&pin.repo_id, &pin.revision_id)?;
        SemanticPolicy::validate_query_against_readiness(pin.manifest_generation, materialized)?;
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
        let query_vector = resolve_query_vector(
            request.query_vector_ref.as_ref(),
            request.query_vector.as_deref(),
            request.query_text.as_deref(),
            "semantic",
            "query_vector_ref",
            "query_vector",
            searcher.as_ref(),
        )?;
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
        let pin = resolve_hybrid_request_pin(self.activation_catalog.as_ref(), request)?;
        let lex_materialized = self.snapshot_lex_materialized(&pin.repo_id, &pin.revision_id)?;
        let sem_materialized = self.snapshot_sem_materialized(&pin.repo_id, &pin.revision_id)?;
        HybridOrchestratorPolicy::validate_joint_readiness(
            pin.manifest_generation,
            lex_materialized,
            sem_materialized,
        )?;

        let lex_searcher =
            self.lex_opener
                .open(&pin.repo_id, &pin.revision_id, pin.manifest_generation)?;
        let sem_searcher =
            self.sem_opener
                .open(&pin.repo_id, &pin.revision_id, pin.manifest_generation)?;
        let lexical_query = lower_lexical_text_query(&request.text_query)?;
        LexicalPolicy::validate_query(&lexical_query)?;
        let internal_top_k = HybridOrchestratorPolicy::over_fetch_top_k(request.top_k);
        let lex_results = lex_searcher.search(&lexical_query, internal_top_k)?;
        let lexical_ids = lex_results
            .iter()
            .map(|candidate| candidate.candidate_id.clone())
            .collect::<BTreeSet<_>>();
        let query_vector = resolve_query_vector(
            request.semantic_vector_ref.as_ref(),
            request.semantic_vector.as_deref(),
            request.semantic_query_text.as_deref(),
            "hybrid",
            "semantic_vector_ref",
            "semantic_vector",
            sem_searcher.as_ref(),
        )?;
        let sem_results =
            sem_searcher.search_scoped(&query_vector, &lexical_ids, internal_top_k)?;
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
        let (pin, domain_request) =
            lower_structural_query_request(self.activation_catalog.as_ref(), request)?;
        let service = StructuralService::new(Arc::clone(&self.structural_producer));
        let mut results = service
            .query(&domain_request)
            .map_err(|err| map_structural_error(&err))?
            .candidates;
        results.truncate(top_k_limit(request.text_query.top_k));
        Ok(SearchPlaneStructuralQueryResponse {
            generation: pin,
            results: project_structural_query_results(results),
        })
    }

    fn bridge(
        &self,
        request: &BridgeQueryRequest,
    ) -> Result<SearchPlaneBridgeQueryResponse, CoreError> {
        let pin = resolve_lexical_request_pin(
            self.activation_catalog.as_ref(),
            &request.text_query,
            SearchPlaneTrackKind::Lexical,
            "bridge",
        )?;
        let materialized = self.snapshot_lex_materialized(&pin.repo_id, &pin.revision_id)?;
        LexicalPolicy::validate_query_against_readiness(pin.manifest_generation, materialized)?;
        let lowered = lower_lexical_text_query(&request.text_query)?;
        LexicalPolicy::validate_query(&lowered)?;
        let searcher =
            self.lex_opener
                .open(&pin.repo_id, &pin.revision_id, pin.manifest_generation)?;
        let candidates = searcher.search(&lowered, request.text_query.top_k)?;
        let packet = export_bridge_candidate_packet(
            request.target,
            BridgeScope::Lexical,
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
        dispatch_query_result(
            self.lexical_query(request),
            SearchPlaneQueryIpcResponse::Text,
        )
    }

    fn dispatch_symbol(&self, request: SymbolQueryRequest) -> SearchPlaneQueryIpcResponse {
        dispatch_query_result(self.symbol(request), SearchPlaneQueryIpcResponse::Symbol)
    }

    fn dispatch_semantic(&self, request: SemanticQueryRequest) -> SearchPlaneQueryIpcResponse {
        dispatch_query_result(
            self.semantic_query(request),
            SearchPlaneQueryIpcResponse::Semantic,
        )
    }

    fn dispatch_hybrid(&self, request: HybridQueryRequest) -> SearchPlaneQueryIpcResponse {
        dispatch_query_result(
            self.hybrid_query(request),
            SearchPlaneQueryIpcResponse::Hybrid,
        )
    }

    fn dispatch_history(&self, request: &HistoryQueryRequest) -> SearchPlaneQueryIpcResponse {
        dispatch_query_result(self.history(request), SearchPlaneQueryIpcResponse::History)
    }

    fn dispatch_structural(&self, request: &StructuralQueryRequest) -> SearchPlaneQueryIpcResponse {
        dispatch_query_result(
            self.structural(request),
            SearchPlaneQueryIpcResponse::Structural,
        )
    }

    fn dispatch_bridge(&self, request: &BridgeQueryRequest) -> SearchPlaneQueryIpcResponse {
        dispatch_query_result(self.bridge(request), SearchPlaneQueryIpcResponse::Bridge)
    }

    fn dispatch_repo_map(&self, request: RepoMapQueryRequest) -> SearchPlaneQueryIpcResponse {
        dispatch_query_result(
            self.repo_map(request),
            SearchPlaneQueryIpcResponse::RepoMapQuery,
        )
    }

    fn dispatch_explain(
        &self,
        request: SearchPlaneExplainQueryRequest,
    ) -> SearchPlaneQueryIpcResponse {
        dispatch_query_result(
            self.explain_query(request),
            SearchPlaneQueryIpcResponse::Explain,
        )
    }

    // QI-RT-02 (in-flight): runtime-metadata query path is defined in the
    // contract but the producer-backed implementation is not wired yet.
    // Fail-closed with a dedicated typed-unavailable code.
    fn dispatch_runtime_metadata(
        &self,
        request: &RuntimeMetadataQueryRequest,
    ) -> SearchPlaneQueryIpcResponse {
        dispatch_query_result(
            self.runtime_metadata(request),
            SearchPlaneQueryIpcResponse::RuntimeMetadata,
        )
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

    fn snapshot_sem_materialized(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
    ) -> Result<Option<ManifestGeneration>, CoreError> {
        let guard = self
            .ledger
            .read()
            .map_err(|err| CoreError::Storage(format!("ledger poisoned: {err}")))?;
        Ok(guard.track_materialized(repo_id, revision_id, SearchPlaneTrackKind::Semantic))
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

fn dispatch_query_result<T>(
    result: Result<T, CoreError>,
    ok: impl FnOnce(T) -> SearchPlaneQueryIpcResponse,
) -> SearchPlaneQueryIpcResponse {
    match result {
        Ok(resp) => ok(resp),
        Err(err) => SearchPlaneQueryIpcResponse::Error(core_error_to_ipc(err)),
    }
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
) -> Result<(GenerationPin, DomainStructuralQueryRequest), CoreError> {
    let lowered = match request.text_query.syntax {
        TextQuerySyntax::Native => lower_lexical_text_query(&request.text_query)?,
        TextQuerySyntax::Sourcegraph => {
            lower_sourcegraph_structural_query_text(&request.text_query.query_text)?
        }
    };
    let pin = resolve_lexical_request_pin(
        activation_catalog,
        &request.text_query,
        SearchPlaneTrackKind::Structural,
        "structural",
    )?;
    let pattern = extract_structural_block(&lowered)?;
    let requested_lang = pattern.lang.clone();
    let (requested_lang, filters) =
        extract_structural_filters(&lowered, requested_lang.as_deref())?;
    Ok((
        pin.clone(),
        DomainStructuralQueryRequest {
            pattern,
            requested_lang,
            filters,
            options: lowered.options,
            generation: GenerationSelector::Pinned(pin),
        },
    ))
}

fn extract_structural_block(query: &LqQuery) -> Result<LqStructuralBlock, CoreError> {
    match &query.expr {
        LqExpr::Leaf(LqLeaf::StructuralBlock(block)) => Ok(block.clone()),
        LqExpr::Empty => Err(structural_invalid_request(
            "query must include exactly one `match { ... }` structural leaf",
        )),
        LqExpr::Leaf(_)
        | LqExpr::Not(_)
        | LqExpr::All(_)
        | LqExpr::Any(_)
        | LqExpr::SemanticVector { .. } => Err(structural_invalid_request(
            "query must lower to exactly one top-level structural block leaf",
        )),
    }
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
    candidates: Vec<StructuralMatchCandidate>,
) -> Vec<quanta_index_contract::StructuralCandidate> {
    candidates
        .into_iter()
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
        LqExpr::SemanticVector { .. } => Err(CoreError::NotImplemented(format!(
            "{plane}: semantic-vector leaves are not executable on this route"
        ))),
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
                    chunk.indexed_text.as_ref(),
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
            chunk.indexed_text.as_ref(),
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
        LqExpr::SemanticVector { .. } => Err(CoreError::NotImplemented(
            "semantic-vector leaves are not executable on this route".to_string(),
        )),
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
        snippet: chunk.snippet.to_string(),
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

fn resolve_semantic_request_pin(
    activation_catalog: &ActivationCatalog,
    request: &SemanticQueryRequest,
) -> Result<GenerationPin, CoreError> {
    let outer_pin = resolve_optional_selection(
        activation_catalog,
        request.generation.clone(),
        request.generation_selector.as_ref(),
        SearchPlaneTrackKind::Semantic,
        "semantic",
    )?;
    let scope_pin = match request.lexical_scope.as_ref() {
        Some(scope) => Some(resolve_lexical_request_pin(
            activation_catalog,
            scope,
            SearchPlaneTrackKind::Lexical,
            "semantic scope",
        )?),
        None => None,
    };
    match (outer_pin, scope_pin) {
        (Some(pin), Some(scope_pin)) if pin != scope_pin => Err(CoreError::InvalidContract(
            "semantic: scope generation does not match semantic request generation".to_string(),
        )),
        (Some(pin), _) | (None, Some(pin)) => Ok(pin),
        (None, None) => Err(CoreError::InvalidContract(
            "semantic: generation pin required".to_string(),
        )),
    }
}

fn resolve_hybrid_request_pin(
    activation_catalog: &ActivationCatalog,
    request: &HybridQueryRequest,
) -> Result<GenerationPin, CoreError> {
    let lexical_pin = resolve_lexical_request_pin(
        activation_catalog,
        &request.text_query,
        SearchPlaneTrackKind::Lexical,
        "hybrid text_query",
    )?;
    let semantic_pin = resolve_optional_selection(
        activation_catalog,
        request.generation.clone(),
        request.generation_selector.as_ref(),
        SearchPlaneTrackKind::Semantic,
        "hybrid",
    )?;
    match semantic_pin {
        Some(pin) if pin != lexical_pin => Err(CoreError::InvalidContract(
            "hybrid: lexical generation does not match semantic generation".to_string(),
        )),
        Some(pin) => Ok(pin),
        None => Ok(lexical_pin),
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

fn resolve_query_vector(
    explicit_ref: Option<&SemanticVectorRef>,
    explicit_legacy: Option<&[f32]>,
    encoded: Option<&str>,
    plane: &str,
    ref_field: &str,
    legacy_field: &str,
    searcher: &dyn quanta_index_core::domains::semantic::SemanticSearcher,
) -> Result<Vec<f32>, CoreError> {
    if explicit_ref.is_some() && explicit_legacy.is_some() {
        return Err(CoreError::InvalidContract(format!(
            "{plane}: `{ref_field}` and `{legacy_field}` are mutually exclusive"
        )));
    }
    match explicit_ref {
        Some(SemanticVectorRef::Inline(query_vector)) => {
            SemanticPolicy::validate_query_vector(query_vector)?;
            Ok(query_vector.clone())
        }
        Some(SemanticVectorRef::Handle(handle)) => searcher.resolve_handle(handle),
        None => match explicit_legacy {
            Some(query_vector) => {
                SemanticPolicy::validate_query_vector(query_vector)?;
                Ok(query_vector.to_vec())
            }
            None => encoded.map_or_else(
                || {
                    // QI-QRY-01 phase 2: no text fallback available. Caller
                    // must supply `{ref_field}`, `{legacy_field}`, or an
                    // encoded text vector. Fail-closed per CLAUDE.md safety
                    // rules.
                    Err(CoreError::InvalidContract(format!(
                        "{plane}: must supply `{ref_field}`, `{legacy_field}`, or text-encoded vector"
                    )))
                },
                |text| {
                    decode_query_text_as_vector(text).map_err(|err| match err {
                        CoreError::Typed { code, message } => CoreError::Typed {
                            code,
                            message: format!("{plane}: {message}"),
                        },
                        err @ (CoreError::InvalidContract(_)
                        | CoreError::NotReady(_)
                        | CoreError::NotImplemented(_)
                        | CoreError::NotFound(_)
                        | CoreError::Storage(_)) => err,
                    })
                },
            ),
        },
    }
}

/// For the reference semantic adapter, the query text is interpreted as a
/// space-separated decimal list of `f32` values.
///
/// Parsing is fail-closed: unparseable, non-finite, or zero-norm vectors are
/// rejected explicitly.
fn decode_query_text_as_vector(text: &str) -> Result<Vec<f32>, CoreError> {
    let mut query_vector: Vec<f32> = Vec::new();
    for token in text.split_whitespace() {
        let value = token.parse::<f32>().map_err(|err| CoreError::Typed {
            code: LexicalErrorCode::SemInvalidVector.as_code_str().to_string(),
            message: format!("semantic: query vector token `{token}` is not a valid f32: {err}"),
        })?;
        if !value.is_finite() {
            return Err(CoreError::Typed {
                code: LexicalErrorCode::SemInvalidVector.as_code_str().to_string(),
                message: format!("semantic: query vector token `{token}` is not finite"),
            });
        }
        query_vector.push(value);
    }
    SemanticPolicy::validate_query_vector(&query_vector)?;
    Ok(query_vector)
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
        ERR_HISTORY_GENERATION_NOT_READY, ERR_HISTORY_PRODUCER_UNAVAILABLE,
        ERR_HISTORY_SHARD_UNAVAILABLE, FailClosedStructuralProducer, SearchPlaneDispatcher,
        make_pin,
    };
    use crate::{ActivationCatalog, Ledger};
    use quanta_index_contract::lex::{CommitRecord, CommitSha, SymbolKindCode, SymbolKindFamily};
    use quanta_index_contract::{
        BridgeQueryRequest, GenerationPin, HistoryQueryRequest, HybridQueryRequest,
        LexicalCandidate, LexicalChannelOp, ManifestGeneration, RepoId, RepoMapDocType,
        RepoMapEntryDto, RepoMapExactnessSummary, RepoMapGraphCoverageClass,
        RepoMapItemIndexAvailability, RepoMapQueryRequest, RepoMapQueryResponse,
        RepoMapRedactionState, RepoMapSnapshotMeta, RepoRelativePath, RevisionId,
        SearchPlaneQueryIpcRequest, SearchPlaneQueryIpcResponse, SemanticQueryRequest,
        SemanticVectorRef, SymbolCandidate, TextQueryRequest, TextQuerySyntax, UpsertCommit,
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
        ledger.semantic_seal(ManifestGeneration::new(9));
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
            None,
        );
        ledger.record_track_seal(
            &repo_id,
            &revision_id,
            SearchPlaneTrackKind::Semantic,
            ManifestGeneration::new(9),
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

        fn resolve_handle(&self, handle: &str) -> Result<Vec<f32>, CoreError> {
            match handle {
                "semantic-handle" => Ok(vec![0.5, 0.5]),
                other => Err(CoreError::Typed {
                    code: "SEM_HANDLE_NOT_FOUND".to_string(),
                    message: format!("semantic: handle `{other}` not found"),
                }),
            }
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
    fn semantic_dispatch_prefers_explicit_query_vector_over_query_text() -> TestResult {
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
                query_text: Some("definitely not a float vector".to_string()),
                query_vector: None,
                query_vector_ref: Some(SemanticVectorRef::Inline(vec![1.0, 0.0, 2.0])),
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
        if search_vectors.as_slice() != [vec![1.0, 0.0, 2.0]] {
            return Err(format!("unexpected semantic vectors: {search_vectors:?}").into());
        }
        if !scoped_vectors.is_empty() {
            return Err(format!("unexpected scoped vectors: {scoped_vectors:?}").into());
        }
        Ok(())
    }

    #[test]
    fn hybrid_dispatch_prefers_explicit_semantic_vector_over_query_text() -> TestResult {
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
                semantic_query_text: Some("not numeric".to_string()),
                semantic_vector: None,
                semantic_vector_ref: Some(SemanticVectorRef::Inline(vec![0.25, 0.75])),
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
        if scoped_vectors.as_slice() != [vec![0.25, 0.75]] {
            return Err(format!("unexpected scoped vectors: {scoped_vectors:?}").into());
        }
        if !search_vectors.is_empty() {
            return Err(format!("unexpected global vectors: {search_vectors:?}").into());
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

    fn structural_match_candidate(id: &str) -> StructuralMatchCandidate {
        StructuralMatchCandidate {
            candidate_id: id.to_string(),
            bindings: vec![StructuralMatchBinding {
                metavariable: "x".to_string(),
                start_byte: 0,
                end_byte: 10,
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
}
