//! Search-plane query orchestration using the in-memory readiness ledger as the
//! source of truth.

use std::collections::BTreeSet;
use std::sync::{Arc, RwLock};

use quanta_index_contract::lex::LexicalErrorCode;
use quanta_index_contract::{
    BridgeQueryRequest, BridgeScope, EngineTouched, GenerationPin, GenerationSelector,
    HistoryQueryRequest, HybridQueryRequest, HybridQueryResponse, LQ_VERSION_TAG, LqExpr, LqLeaf,
    LqOptions, LqQuery, LqSpan, ManifestGeneration, PlannerStage, PlannerTraceEntry, RepoId,
    RepoMapQueryRequest, RepoMapQueryResponse, RevisionId, SearchExplanation,
    SearchPlaneBridgeQueryResponse, SearchPlaneExplainQueryRequest,
    SearchPlaneExplainQueryResponse, SearchPlaneHistoryQueryResponse, SearchPlaneIpcError,
    SearchPlaneQueryIpcRequest, SearchPlaneQueryIpcResponse, SearchPlaneSourcegraphQueryRequest,
    SearchPlaneSourcegraphQueryResponse, SearchPlaneStructuralQueryResponse, SearchPlaneTrackKind,
    SemanticQueryRequest, SemanticQueryResponse, SemanticVectorRef, StructuralBinding,
    StructuralQueryRequest, SymbolQueryRequest, SymbolQueryResponse, TextQueryRequest,
    TextQueryResponse, TextQuerySyntax,
};
use quanta_index_core::domains::structural::{
    StructuralProducerPort, StructuralQueryRequest as DomainStructuralQueryRequest,
    StructuralReadiness,
};
use quanta_index_core::{
    CoreError, ExplainQueryPort, HybridOrchestratorPolicy, HybridQueryPort, LexicalIndexOpenPort,
    LexicalPolicy, LexicalQueryPort, RepoMapPolicy, RepoMapQueryPort, SemanticIndexOpenPort,
    SemanticPolicy, SemanticQueryPort,
};
use quanta_index_lq_bridge::{
    BridgeErrorCode, SUPPORTED_SG_VERSION, SourcegraphVersionTag, export_bridge_candidate_packet,
};

use crate::{ActivationCatalog, Ledger, lower_lexical_text_query};

const ERR_INVALID: &str = "INVALID_REQUEST";
const ERR_NOT_READY: &str = "NOT_READY";
const ERR_NOT_FOUND: &str = "NOT_FOUND";
const ERR_NOT_IMPLEMENTED: &str = "NOT_IMPLEMENTED";
const ERR_INTERNAL: &str = "INTERNAL";

pub struct SearchPlaneDispatcher {
    lex_opener: Arc<dyn LexicalIndexOpenPort + Send + Sync>,
    sem_opener: Arc<dyn SemanticIndexOpenPort + Send + Sync>,
    repo_map_query: Arc<dyn RepoMapQueryPort + Send + Sync>,
    /// Held at the composition seam so the runtime can wire a producer once
    /// the structural-block AST lands on the IPC wire. Today the dispatcher
    /// fail-closes structural requests with `NotImplemented` and never
    /// consults this port; the field's job is to keep the DIP-correct
    /// constructor signature stable across the cutover so
    /// `searchd::app::runtime` does not have to change.
    _structural_producer: Arc<dyn StructuralProducerPort + Send + Sync>,
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
            _structural_producer: structural_producer,
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
        let materialized = self.snapshot_lex_seal()?;
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
        let materialized = self.snapshot_lex_seal()?;
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

    fn sourcegraph(
        &self,
        request: &SearchPlaneSourcegraphQueryRequest,
    ) -> Result<SearchPlaneSourcegraphQueryResponse, CoreError> {
        let version = SourcegraphVersionTag::new(request.sg_version.as_ref()).map_err(|err| {
            CoreError::Typed {
                code: BridgeErrorCode::BridgeVersionPin.as_code_str().to_string(),
                message: format!("sourcegraph: {err}"),
            }
        })?;
        if version.as_str() != SUPPORTED_SG_VERSION {
            return Err(CoreError::Typed {
                code: BridgeErrorCode::BridgeVersionPin.as_code_str().to_string(),
                message: format!(
                    "sourcegraph: unsupported sg_version `{}`; supported `{SUPPORTED_SG_VERSION}`",
                    version.as_str()
                ),
            });
        }
        let generation = request.generation.clone().ok_or_else(|| {
            CoreError::InvalidContract("sourcegraph: generation pin required".to_string())
        })?;
        let lexical_request = TextQueryRequest {
            syntax: TextQuerySyntax::Sourcegraph,
            query_text: request.source_syntax.to_string(),
            generation: Some(generation),
            generation_selector: None,
            top_k: request.top_k,
        };
        let response = self.lexical(&lexical_request)?;
        Ok(SearchPlaneSourcegraphQueryResponse {
            generation: response.generation,
            results: response.results,
        })
    }

    fn semantic(&self, request: &SemanticQueryRequest) -> Result<SemanticQueryResponse, CoreError> {
        SemanticPolicy::validate_top_k(request.top_k)?;
        let pin = resolve_semantic_request_pin(self.activation_catalog.as_ref(), request)?;
        let materialized = self.snapshot_sem_seal()?;
        SemanticPolicy::validate_query_against_readiness(pin.manifest_generation, materialized)?;
        let scope_candidate_ids = if let Some(scope) = request.lexical_scope.as_ref() {
            let lowered_scope = lower_lexical_text_query(scope)?;
            LexicalPolicy::validate_query(&lowered_scope)?;
            let lex_materialized = self.snapshot_lex_seal()?;
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
        let explanation = build_semantic_response_explanation(
            scope_candidate_ids.as_ref().map_or(0, BTreeSet::len),
            scope_candidate_ids.is_some(),
            results.len(),
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
        let lex_seal = self.snapshot_lex_seal()?;
        let sem_seal = self.snapshot_sem_seal()?;
        HybridOrchestratorPolicy::validate_joint_readiness(
            pin.manifest_generation,
            lex_seal,
            sem_seal,
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
        let fused = HybridOrchestratorPolicy::fuse_rrf(&lex_results, &sem_results, request.top_k);
        let explanation = build_hybrid_response_explanation(
            lexical_ids.len(),
            lex_results.len(),
            sem_results.len(),
            fused.len(),
            internal_top_k,
        );
        Ok(HybridQueryResponse {
            generation: pin,
            results: fused,
            explanation,
        })
    }

    fn history(
        &self,
        _request: HistoryQueryRequest,
    ) -> Result<SearchPlaneHistoryQueryResponse, CoreError> {
        Err(CoreError::Typed {
            code: "HISTORY_PRODUCER_UNAVAILABLE".to_string(),
            message:
                "history: producer commit/diff channel ops are not wired in this repo-first closeout"
                    .to_string(),
        })
    }

    /// Structural query dispatch.
    ///
    /// The IPC `StructuralQueryRequest` carries only a wrapped text query
    /// today; the structural-block AST is not yet on the wire. Until the
    /// owner ticket lands the wire shape, this path is fail-closed with
    /// `NotImplemented` — the dispatcher must not fabricate an empty
    /// `LqStructuralBlock` and route it through the producer (that would be
    /// heuristic authority where the caller's `query_text` is discarded).
    fn structural(
        &self,
        _request: &StructuralQueryRequest,
    ) -> Result<SearchPlaneStructuralQueryResponse, CoreError> {
        Err(CoreError::NotImplemented(
            "structural: query pattern is not yet on the IPC wire — pending owner ticket"
                .to_string(),
        ))
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
        let materialized = self.snapshot_lex_seal()?;
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
        let materialized = self.snapshot_lex_seal()?;
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
            SearchPlaneQueryIpcRequest::History(req) => self.dispatch_history(req),
            SearchPlaneQueryIpcRequest::Structural(req) => self.dispatch_structural(&req),
            SearchPlaneQueryIpcRequest::Bridge(req) => self.dispatch_bridge(&req),
            SearchPlaneQueryIpcRequest::RepoMapQuery(req) => self.dispatch_repo_map(req),
            SearchPlaneQueryIpcRequest::Explain(req) => self.dispatch_explain(req),
            SearchPlaneQueryIpcRequest::Sourcegraph(req) => self.dispatch_sourcegraph(&req),
            SearchPlaneQueryIpcRequest::RuntimeMetadata(req) => self.dispatch_runtime_metadata(req),
        }
    }

    fn dispatch_text(&self, request: TextQueryRequest) -> SearchPlaneQueryIpcResponse {
        match self.lexical_query(request) {
            Ok(resp) => SearchPlaneQueryIpcResponse::Text(resp),
            Err(err) => SearchPlaneQueryIpcResponse::Error(core_error_to_ipc(err)),
        }
    }

    fn dispatch_symbol(&self, request: SymbolQueryRequest) -> SearchPlaneQueryIpcResponse {
        match self.symbol(request) {
            Ok(resp) => SearchPlaneQueryIpcResponse::Symbol(resp),
            Err(err) => SearchPlaneQueryIpcResponse::Error(core_error_to_ipc(err)),
        }
    }

    fn dispatch_semantic(&self, request: SemanticQueryRequest) -> SearchPlaneQueryIpcResponse {
        match self.semantic_query(request) {
            Ok(resp) => SearchPlaneQueryIpcResponse::Semantic(resp),
            Err(err) => SearchPlaneQueryIpcResponse::Error(core_error_to_ipc(err)),
        }
    }

    fn dispatch_hybrid(&self, request: HybridQueryRequest) -> SearchPlaneQueryIpcResponse {
        match self.hybrid_query(request) {
            Ok(resp) => SearchPlaneQueryIpcResponse::Hybrid(resp),
            Err(err) => SearchPlaneQueryIpcResponse::Error(core_error_to_ipc(err)),
        }
    }

    fn dispatch_history(&self, request: HistoryQueryRequest) -> SearchPlaneQueryIpcResponse {
        match self.history(request) {
            Ok(resp) => SearchPlaneQueryIpcResponse::History(resp),
            Err(err) => SearchPlaneQueryIpcResponse::Error(core_error_to_ipc(err)),
        }
    }

    fn dispatch_structural(&self, request: &StructuralQueryRequest) -> SearchPlaneQueryIpcResponse {
        match self.structural(request) {
            Ok(resp) => SearchPlaneQueryIpcResponse::Structural(resp),
            Err(err) => SearchPlaneQueryIpcResponse::Error(core_error_to_ipc(err)),
        }
    }

    fn dispatch_bridge(&self, request: &BridgeQueryRequest) -> SearchPlaneQueryIpcResponse {
        match self.bridge(request) {
            Ok(resp) => SearchPlaneQueryIpcResponse::Bridge(resp),
            Err(err) => SearchPlaneQueryIpcResponse::Error(core_error_to_ipc(err)),
        }
    }

    fn dispatch_repo_map(&self, request: RepoMapQueryRequest) -> SearchPlaneQueryIpcResponse {
        match self.repo_map(request) {
            Ok(resp) => SearchPlaneQueryIpcResponse::RepoMapQuery(resp),
            Err(err) => SearchPlaneQueryIpcResponse::Error(core_error_to_ipc(err)),
        }
    }

    fn dispatch_explain(
        &self,
        request: SearchPlaneExplainQueryRequest,
    ) -> SearchPlaneQueryIpcResponse {
        match self.explain_query(request) {
            Ok(resp) => SearchPlaneQueryIpcResponse::Explain(resp),
            Err(err) => SearchPlaneQueryIpcResponse::Error(core_error_to_ipc(err)),
        }
    }

    fn dispatch_sourcegraph(
        &self,
        request: &SearchPlaneSourcegraphQueryRequest,
    ) -> SearchPlaneQueryIpcResponse {
        match self.sourcegraph(request) {
            Ok(resp) => SearchPlaneQueryIpcResponse::Sourcegraph(resp),
            Err(err) => SearchPlaneQueryIpcResponse::Error(core_error_to_ipc(err)),
        }
    }

    // QI-RT-02 (in-flight): runtime-metadata query path is defined in the
    // contract but the dispatcher implementation is not yet wired.
    // Fail-closed with NOT_IMPLEMENTED per CLAUDE.md.
    fn dispatch_runtime_metadata(
        &self,
        _request: quanta_index_contract::RuntimeMetadataQueryRequest,
    ) -> SearchPlaneQueryIpcResponse {
        SearchPlaneQueryIpcResponse::Error(core_error_to_ipc(CoreError::NotImplemented(
            "query: runtime-metadata path awaiting QI-RT-02 wiring".to_string(),
        )))
    }

    fn snapshot_lex_seal(&self) -> Result<Option<ManifestGeneration>, CoreError> {
        let guard = self
            .ledger
            .read()
            .map_err(|err| CoreError::Storage(format!("ledger poisoned: {err}")))?;
        Ok(guard.lexical_sealed())
    }

    fn snapshot_sem_seal(&self) -> Result<Option<ManifestGeneration>, CoreError> {
        let guard = self
            .ledger
            .read()
            .map_err(|err| CoreError::Storage(format!("ledger poisoned: {err}")))?;
        Ok(guard.semantic_sealed())
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

/// Production stand-in for the structural producer port.
///
/// The structural dispatch path is fail-closed inside the dispatcher today
/// (`structural()` returns `NotImplemented` until the wire shape lands), so
/// this adapter is wired by the composition root purely to satisfy the
/// `StructuralProducerPort` seam. `readiness` reports
/// `ParseTreeProducerUnavailable` to keep the port honest.
pub struct FailClosedStructuralProducer;

impl StructuralProducerPort for FailClosedStructuralProducer {
    fn readiness(&self, _request: &DomainStructuralQueryRequest) -> StructuralReadiness {
        StructuralReadiness::ParseTreeProducerUnavailable
    }

    fn execute(
        &self,
        _request: &DomainStructuralQueryRequest,
    ) -> Result<Vec<StructuralBinding>, quanta_index_core::domains::structural::StructuralError>
    {
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
        early_stop_reason: None,
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
        early_stop_reason: None,
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

    use super::{FailClosedStructuralProducer, SearchPlaneDispatcher, make_pin};
    use crate::{ActivationCatalog, Ledger};
    use quanta_index_contract::{
        BridgeQueryRequest, GenerationPin, HybridQueryRequest, LexicalCandidate,
        ManifestGeneration, RepoId, RepoMapEntryDto, RepoMapQueryRequest, RepoMapQueryResponse,
        RepoMapSnapshotMeta, RepoRelativePath, RevisionId, SearchPlaneQueryIpcRequest,
        SearchPlaneQueryIpcResponse, SearchPlaneSourcegraphQueryRequest, SemanticQueryRequest,
        SemanticVectorRef, TextQueryRequest, TextQuerySyntax,
    };
    use quanta_index_core::{
        CoreError, LexicalIndexOpenPort, LexicalSearcher, RepoMapQueryPort, SemanticIndexOpenPort,
        SemanticSearcher,
    };
    use quanta_index_lq_bridge::SUPPORTED_SG_VERSION;
    use tempfile::tempdir;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

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
                    item_index_availability: "available".to_string(),
                    graph_coverage_class: "full".to_string(),
                    exactness_summary: "exact".to_string(),
                },
                entries: vec![RepoMapEntryDto {
                    subject_identity: "src/lib.rs::Owner".to_string(),
                    subject_doc_type: "Symbol".to_string(),
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
                    redaction_state: "Unredacted".to_string(),
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
                subject_doc_type: "Symbol".to_string(),
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
            | SearchPlaneQueryIpcResponse::Sourcegraph(_)
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
        ledger.lexical_seal(ManifestGeneration::new(9));
        ledger.semantic_seal(ManifestGeneration::new(9));
        Arc::new(RwLock::new(ledger))
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
        ) -> Result<Vec<LexicalCandidate>, CoreError> {
            Ok(self.results.clone())
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
        ) -> Result<Vec<LexicalCandidate>, CoreError> {
            self.state
                .lock()
                .map_err(|err| CoreError::Storage(format!("lexical state poisoned: {err}")))?
                .symbol_top_ks
                .push(top_k);
            Ok(self.results.clone())
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
            | SearchPlaneQueryIpcResponse::Sourcegraph(_)
            | quanta_index_contract::SearchPlaneQueryIpcResponse::RuntimeMetadata(_)) => {
                return Err(format!("expected Error response, got {other:?}").into());
            }
        }
        Ok(())
    }

    #[test]
    fn sourcegraph_dispatch_returns_dedicated_sourcegraph_payload() -> TestResult {
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
        let response = dispatcher.dispatch(SearchPlaneQueryIpcRequest::Sourcegraph(
            SearchPlaneSourcegraphQueryRequest {
                source_syntax: "repo:repo-map-ipc alpha".into(),
                sg_version: SUPPORTED_SG_VERSION.into(),
                generation: Some(pin.clone()),
                top_k: 2,
            },
        ));

        match response {
            SearchPlaneQueryIpcResponse::Sourcegraph(sourcegraph) => {
                if sourcegraph.generation != pin {
                    return Err("sourcegraph response did not echo request pin".into());
                }
                if sourcegraph.results.len() != 2 {
                    return Err(format!(
                        "expected two sourcegraph results, got {}",
                        sourcegraph.results.len()
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
            | SearchPlaneQueryIpcResponse::Bridge(_)
            | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
            | SearchPlaneQueryIpcResponse::Explain(_)
            | SearchPlaneQueryIpcResponse::Error(_)
            | SearchPlaneQueryIpcResponse::RuntimeMetadata(_)) => {
                return Err(format!("expected Sourcegraph response, got {other:?}").into());
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
            | SearchPlaneQueryIpcResponse::Sourcegraph(_)
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
            | SearchPlaneQueryIpcResponse::Sourcegraph(_)
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
            | SearchPlaneQueryIpcResponse::Sourcegraph(_)
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

    use quanta_index_contract::StructuralBinding;
    use quanta_index_core::domains::structural::{
        StructuralError, StructuralProducerPort, StructuralQueryRequest, StructuralReadiness,
    };

    /// Test producer that records how many times `readiness` was consulted
    /// and lets a test choose which readiness value is returned.
    struct RecordingStructuralProducer {
        readiness: StructuralReadiness,
        readiness_calls: AtomicUsize,
    }

    impl RecordingStructuralProducer {
        fn new(readiness: StructuralReadiness) -> Self {
            Self {
                readiness,
                readiness_calls: AtomicUsize::new(0),
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
        ) -> Result<Vec<StructuralBinding>, StructuralError> {
            // Unreachable when the service gates on `Ready` and the test
            // injects a non-`Ready` readiness; defensive typed error keeps
            // the producer fail-closed.
            Err(StructuralError::ProducerExecution(
                "RecordingStructuralProducer.execute should not be reached".to_string(),
            ))
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
            | SearchPlaneQueryIpcResponse::Sourcegraph(_)
            | SearchPlaneQueryIpcResponse::RuntimeMetadata(_)) => {
                Err(format!("expected Error response, got {other:?}"))
            }
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
            | SearchPlaneQueryIpcResponse::Error(_)
            | SearchPlaneQueryIpcResponse::Sourcegraph(_)) => {
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
            | SearchPlaneQueryIpcResponse::Sourcegraph(_)
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

    /// Structural dispatch is fail-closed at the dispatcher boundary until
    /// the structural-block AST lands on the IPC wire.
    ///
    /// The producer port is NOT consulted. The dispatcher used to fabricate
    /// an empty pattern and route it through the domain service, which is
    /// heuristic authority because the caller's `query_text` was discarded.
    /// Now the wire code is the generic `NOT_IMPLEMENTED` regardless of what
    /// the producer would have reported.
    #[test]
    fn structural_dispatch_is_fail_closed_with_not_implemented() -> TestResult {
        let producer = Arc::new(RecordingStructuralProducer::new(StructuralReadiness::Ready));
        let dispatcher = structural_dispatcher_with_producer(Arc::clone(&producer))?;

        let response = dispatcher.dispatch(SearchPlaneQueryIpcRequest::Structural(
            quanta_index_contract::StructuralQueryRequest {
                text_query: TextQueryRequest {
                    syntax: TextQuerySyntax::Native,
                    query_text: "match { Symbol }".to_string(),
                    generation: Some(ready_pin()),
                    generation_selector: None,
                    top_k: 4,
                },
            },
        ));

        let (code, _message) =
            ipc_error_from(response).map_err(Box::<dyn std::error::Error>::from)?;
        if code != "NOT_IMPLEMENTED" {
            return Err(format!("expected NOT_IMPLEMENTED, got {code}").into());
        }
        let consulted = producer.readiness_calls.load(Ordering::SeqCst);
        if consulted != 0 {
            return Err(format!(
                "expected the structural producer to NOT be consulted (dispatcher fail-closed), got {consulted} call(s)"
            )
            .into());
        }
        Ok(())
    }
}
