//! `QueryDispatcher` impl that routes IPC requests to lexical / semantic /
//! hybrid / explain handlers using the in-memory ledger as the readiness
//! source of truth.

use std::collections::BTreeSet;
use std::sync::{Arc, RwLock};

use quanta_index_contract::lex::LexicalErrorCode;
use quanta_index_contract::{
    BridgeScope, EngineTouched, GenerationPin, LQ_VERSION_TAG, LqExpr, LqLeaf, LqOptions, LqQuery,
    LqSpan, ManifestGeneration, PlannerStage, PlannerTraceEntry, RepoId,
    RepoMapActivateGenerationRequestV1, RepoMapMutationAckV1, RepoMapQueryRequestV1,
    RepoMapQueryResponseV1, RepoMapSourceBundleV1, RevisionId, SearchExplanation,
    SearchPlaneBridgeQueryRequest, SearchPlaneBridgeQueryResponse, SearchPlaneExplainQueryRequest,
    SearchPlaneExplainQueryResponse, SearchPlaneHistoryQueryRequest,
    SearchPlaneHistoryQueryResponse, SearchPlaneHybridQueryRequest, SearchPlaneHybridQueryResponse,
    SearchPlaneIpcError, SearchPlaneIpcRequest, SearchPlaneIpcResponse,
    SearchPlaneLexicalQueryResponse, SearchPlaneLexicalTextQueryRequestV2,
    SearchPlaneSemanticQueryRequest, SearchPlaneSemanticQueryResponse,
    SearchPlaneStructuralQueryRequest, SearchPlaneStructuralQueryResponse, SemanticVectorRef,
};
use quanta_index_core::domains::lexical::lower_lexical_text_query;
use quanta_index_core::{
    CoreError, ExplainQueryPort, HybridOrchestratorPolicy, HybridQueryPort, LexicalIndexOpenPort,
    LexicalPolicy, LexicalQueryPort, RepoMapBundleIngestPort, RepoMapGenerationActivatePort,
    RepoMapPolicy, RepoMapQueryPort, SemanticIndexOpenPort, SemanticPolicy, SemanticQueryPort,
};
use quanta_index_ipc::QueryDispatcher;
use quanta_index_lq_bridge::export_bridge_candidate_packet;

use crate::runtime::Ledger;

const ERR_INVALID: &str = "INVALID_REQUEST";
const ERR_NOT_READY: &str = "NOT_READY";
const ERR_NOT_FOUND: &str = "NOT_FOUND";
const ERR_NOT_IMPLEMENTED: &str = "NOT_IMPLEMENTED";
const ERR_INTERNAL: &str = "INTERNAL";

pub struct SearchPlaneDispatcher {
    lex_opener: Arc<dyn LexicalIndexOpenPort + Send + Sync>,
    sem_opener: Arc<dyn SemanticIndexOpenPort + Send + Sync>,
    repo_map_ingest: Arc<dyn RepoMapBundleIngestPort + Send + Sync>,
    repo_map_activate: Arc<dyn RepoMapGenerationActivatePort + Send + Sync>,
    repo_map_query: Arc<dyn RepoMapQueryPort + Send + Sync>,
    ledger: Arc<RwLock<Ledger>>,
}

impl SearchPlaneDispatcher {
    #[must_use]
    pub fn new(
        lex_opener: Arc<dyn LexicalIndexOpenPort + Send + Sync>,
        sem_opener: Arc<dyn SemanticIndexOpenPort + Send + Sync>,
        repo_map_ingest: Arc<dyn RepoMapBundleIngestPort + Send + Sync>,
        repo_map_activate: Arc<dyn RepoMapGenerationActivatePort + Send + Sync>,
        repo_map_query: Arc<dyn RepoMapQueryPort + Send + Sync>,
        ledger: Arc<RwLock<Ledger>>,
    ) -> Self {
        Self {
            lex_opener,
            sem_opener,
            repo_map_ingest,
            repo_map_activate,
            repo_map_query,
            ledger,
        }
    }

    fn lexical(
        &self,
        request: SearchPlaneLexicalTextQueryRequestV2,
    ) -> Result<SearchPlaneLexicalQueryResponse, CoreError> {
        let lowered = lower_lexical_text_query(&request)?;
        LexicalPolicy::validate_query(&lowered)?;
        let pin = request.generation.ok_or_else(|| {
            CoreError::InvalidContract("lexical: generation pin required".to_string())
        })?;
        let materialized = self.snapshot_lex_seal()?;
        LexicalPolicy::validate_query_against_readiness(pin.manifest_generation, materialized)?;
        let searcher =
            self.lex_opener
                .open(&pin.repo_id, &pin.revision_id, pin.manifest_generation)?;
        let results = searcher.search(&lowered, default_top_k())?;
        Ok(SearchPlaneLexicalQueryResponse {
            generation: pin,
            results,
        })
    }

    fn semantic(
        &self,
        request: &SearchPlaneSemanticQueryRequest,
    ) -> Result<SearchPlaneSemanticQueryResponse, CoreError> {
        SemanticPolicy::validate_top_k(request.top_k)?;
        let pin = resolve_pin_with_scope(
            request.generation.clone(),
            request.lexical_scope.as_ref(),
            "semantic",
        )?;
        if let Some(scope) = request.lexical_scope.as_ref() {
            ensure_scope_generation_matches(&pin, scope, "semantic")?;
        }
        let materialized = self.snapshot_sem_seal()?;
        SemanticPolicy::validate_query_against_readiness(pin.manifest_generation, materialized)?;
        let lexical_scope_ids = if let Some(scope) = request.lexical_scope.as_ref() {
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
            &request.query_text,
            "semantic",
            "query_vector_ref",
            "query_vector",
            searcher.as_ref(),
        )?;
        let results = if let Some(scope_ids) = lexical_scope_ids.as_ref() {
            searcher.search_scoped(&query_vector, scope_ids, request.top_k)?
        } else {
            searcher.search(&query_vector, request.top_k)?
        };
        let explanation = build_semantic_response_explanation(
            lexical_scope_ids.as_ref().map_or(0, BTreeSet::len),
            lexical_scope_ids.is_some(),
            results.len(),
        );
        Ok(SearchPlaneSemanticQueryResponse {
            generation: pin,
            results,
            explanation,
        })
    }

    fn hybrid(
        &self,
        request: &SearchPlaneHybridQueryRequest,
    ) -> Result<SearchPlaneHybridQueryResponse, CoreError> {
        HybridOrchestratorPolicy::validate_top_k(request.top_k)?;
        let pin =
            resolve_pin_with_scope(request.generation.clone(), Some(&request.lexical), "hybrid")?;
        ensure_scope_generation_matches(&pin, &request.lexical, "hybrid")?;
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
        let lexical_query = lower_lexical_text_query(&request.lexical)?;
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
            &request.semantic_query_text,
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
        Ok(SearchPlaneHybridQueryResponse {
            generation: pin,
            results: fused,
            explanation,
        })
    }

    fn history(
        &self,
        _request: SearchPlaneHistoryQueryRequest,
    ) -> Result<SearchPlaneHistoryQueryResponse, CoreError> {
        Err(CoreError::Typed {
            code: "HISTORY_PRODUCER_UNAVAILABLE".to_string(),
            message:
                "history: producer commit/diff channel ops are not wired in this repo-first closeout"
                    .to_string(),
        })
    }

    fn structural(
        &self,
        _request: SearchPlaneStructuralQueryRequest,
    ) -> Result<SearchPlaneStructuralQueryResponse, CoreError> {
        Err(CoreError::Typed {
            code: "STR_PRODUCER_PARSE_TREE_UNAVAILABLE".to_string(),
            message:
                "structural: parse-tree producer ops are unavailable; runtime remains fail-closed"
                    .to_string(),
        })
    }

    fn bridge(
        &self,
        request: &SearchPlaneBridgeQueryRequest,
    ) -> Result<SearchPlaneBridgeQueryResponse, CoreError> {
        let pin = request.lexical.generation.clone().ok_or_else(|| {
            CoreError::InvalidContract("bridge: generation pin required".to_string())
        })?;
        let materialized = self.snapshot_lex_seal()?;
        LexicalPolicy::validate_query_against_readiness(pin.manifest_generation, materialized)?;
        let lowered = lower_lexical_text_query(&request.lexical)?;
        LexicalPolicy::validate_query(&lowered)?;
        let searcher =
            self.lex_opener
                .open(&pin.repo_id, &pin.revision_id, pin.manifest_generation)?;
        let candidates = searcher.search(&lowered, default_top_k())?;
        let packet = export_bridge_candidate_packet(
            request.target,
            BridgeScope::Lexical,
            &pin,
            &request.lexical,
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

    fn repo_map(
        &self,
        request: RepoMapQueryRequestV1,
    ) -> Result<RepoMapQueryResponseV1, CoreError> {
        RepoMapPolicy::validate_query(&request)?;
        self.repo_map_query.query(request)
    }

    fn repo_map_ingest(
        &self,
        bundle: RepoMapSourceBundleV1,
    ) -> Result<RepoMapMutationAckV1, CoreError> {
        self.repo_map_ingest.ingest_bundle(&bundle)?;
        Ok(RepoMapMutationAckV1 {
            repo_id: bundle.repo_id,
            revision_id: bundle.revision_id,
            manifest_generation: bundle.manifest_generation,
        })
    }

    fn repo_map_activate(
        &self,
        request: RepoMapActivateGenerationRequestV1,
    ) -> Result<RepoMapMutationAckV1, CoreError> {
        self.repo_map_activate.activate_generation(&request)?;
        Ok(RepoMapMutationAckV1 {
            repo_id: request.repo_id,
            revision_id: request.revision_id,
            manifest_generation: request.manifest_generation,
        })
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
    fn lexical_query(
        &self,
        request: SearchPlaneLexicalTextQueryRequestV2,
    ) -> Result<SearchPlaneLexicalQueryResponse, CoreError> {
        self.lexical(request)
    }
}

impl SemanticQueryPort for SearchPlaneDispatcher {
    fn semantic_query(
        &self,
        request: SearchPlaneSemanticQueryRequest,
    ) -> Result<SearchPlaneSemanticQueryResponse, CoreError> {
        self.semantic(&request)
    }
}

impl HybridQueryPort for SearchPlaneDispatcher {
    fn hybrid_query(
        &self,
        request: SearchPlaneHybridQueryRequest,
    ) -> Result<SearchPlaneHybridQueryResponse, CoreError> {
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

impl QueryDispatcher for SearchPlaneDispatcher {
    fn dispatch(&self, request: SearchPlaneIpcRequest) -> SearchPlaneIpcResponse {
        match request {
            SearchPlaneIpcRequest::Lexical(req) => match self.lexical_query(req) {
                Ok(resp) => SearchPlaneIpcResponse::Lexical(resp),
                Err(err) => SearchPlaneIpcResponse::Error(core_error_to_ipc(err)),
            },
            SearchPlaneIpcRequest::Semantic(req) => match self.semantic_query(req) {
                Ok(resp) => SearchPlaneIpcResponse::Semantic(resp),
                Err(err) => SearchPlaneIpcResponse::Error(core_error_to_ipc(err)),
            },
            SearchPlaneIpcRequest::Hybrid(req) => match self.hybrid_query(req) {
                Ok(resp) => SearchPlaneIpcResponse::Hybrid(resp),
                Err(err) => SearchPlaneIpcResponse::Error(core_error_to_ipc(err)),
            },
            SearchPlaneIpcRequest::History(req) => match self.history(req) {
                Ok(resp) => SearchPlaneIpcResponse::History(resp),
                Err(err) => SearchPlaneIpcResponse::Error(core_error_to_ipc(err)),
            },
            SearchPlaneIpcRequest::Structural(req) => match self.structural(req) {
                Ok(resp) => SearchPlaneIpcResponse::Structural(resp),
                Err(err) => SearchPlaneIpcResponse::Error(core_error_to_ipc(err)),
            },
            SearchPlaneIpcRequest::Bridge(req) => match self.bridge(&req) {
                Ok(resp) => SearchPlaneIpcResponse::Bridge(resp),
                Err(err) => SearchPlaneIpcResponse::Error(core_error_to_ipc(err)),
            },
            SearchPlaneIpcRequest::RepoMapIngest(bundle) => match self.repo_map_ingest(bundle) {
                Ok(resp) => SearchPlaneIpcResponse::RepoMapMutationAck(resp),
                Err(err) => SearchPlaneIpcResponse::Error(core_error_to_ipc(err)),
            },
            SearchPlaneIpcRequest::RepoMapActivate(request) => {
                match self.repo_map_activate(request) {
                    Ok(resp) => SearchPlaneIpcResponse::RepoMapMutationAck(resp),
                    Err(err) => SearchPlaneIpcResponse::Error(core_error_to_ipc(err)),
                }
            }
            SearchPlaneIpcRequest::RepoMapQuery(req) => match self.repo_map(req) {
                Ok(resp) => SearchPlaneIpcResponse::RepoMapQuery(resp),
                Err(err) => SearchPlaneIpcResponse::Error(core_error_to_ipc(err)),
            },
            SearchPlaneIpcRequest::Explain(req) => match self.explain_query(req) {
                Ok(resp) => SearchPlaneIpcResponse::Explain(resp),
                Err(err) => SearchPlaneIpcResponse::Error(core_error_to_ipc(err)),
            },
        }
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

fn resolve_pin_with_scope(
    generation: Option<GenerationPin>,
    scope: Option<&SearchPlaneLexicalTextQueryRequestV2>,
    plane: &str,
) -> Result<GenerationPin, CoreError> {
    match (generation, scope.and_then(|value| value.generation.clone())) {
        (Some(pin), Some(scope_pin)) if pin != scope_pin => Err(CoreError::InvalidContract(
            format!("{plane}: outer generation pin does not match lexical scope generation pin"),
        )),
        (Some(pin), _) | (None, Some(pin)) => Ok(pin),
        (None, None) => Err(CoreError::InvalidContract(format!(
            "{plane}: generation pin required"
        ))),
    }
}

fn ensure_scope_generation_matches(
    pin: &GenerationPin,
    scope: &SearchPlaneLexicalTextQueryRequestV2,
    plane: &str,
) -> Result<(), CoreError> {
    if let Some(scope_pin) = scope.generation.as_ref()
        && scope_pin != pin
    {
        return Err(CoreError::InvalidContract(format!(
            "{plane}: lexical scope generation pin does not match request generation pin"
        )));
    }
    Ok(())
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
    lexical_scope_size: usize,
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
            detail: format!("semantic.scope.lexical_candidates={lexical_scope_size}"),
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
            "semantic scoped query returned {result_count} candidates from lexical scope of {lexical_scope_size}"
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

/// For the reference semantic adapter, the query text is interpreted as a
/// space-separated decimal list of `f32` values.
///
/// Parsing is fail-closed: unparseable, non-finite, or zero-norm vectors are
/// rejected explicitly.
fn resolve_query_vector(
    explicit_ref: Option<&SemanticVectorRef>,
    explicit_legacy: Option<&[f32]>,
    encoded: &str,
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
            Ok(query_vector.to_vec())
        }
        Some(SemanticVectorRef::Handle(handle)) => searcher.resolve_handle(handle),
        None => match explicit_legacy {
            Some(query_vector) => {
                SemanticPolicy::validate_query_vector(query_vector)?;
                Ok(query_vector.to_vec())
            }
            None => decode_query_text_as_vector(encoded).map_err(|err| match err {
                CoreError::Typed { code, message } => CoreError::Typed {
                    code,
                    message: format!("{plane}: {message}"),
                },
                other @ (CoreError::InvalidContract(_)
                | CoreError::NotReady(_)
                | CoreError::NotImplemented(_)
                | CoreError::NotFound(_)
                | CoreError::Storage(_)) => other,
            }),
        },
    }
}

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
    use std::sync::{Arc, RwLock};

    use quanta_index_contract::{
        ManifestGeneration, RepoId, RepoMapActivateGenerationRequestV1, RepoMapEntryDtoV1,
        RepoMapMutationAckV1, RepoMapQueryRequestV1, RepoMapQueryResponseV1, RepoMapSnapshotMetaV1,
        RepoMapSourceBundleV1, RevisionId, SearchPlaneIpcRequest, SearchPlaneIpcResponse,
    };
    use quanta_index_core::{
        CoreError, LexicalIndexOpenPort, LexicalSearcher, RepoMapBundleIngestPort,
        RepoMapGenerationActivatePort, RepoMapQueryPort, SemanticIndexOpenPort, SemanticSearcher,
    };
    use quanta_index_ipc::QueryDispatcher;

    use super::SearchPlaneDispatcher;
    use crate::runtime::Ledger;

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
    struct StubRepoMapIngestPort;
    struct StubRepoMapActivatePort;

    impl RepoMapQueryPort for StubRepoMapQueryPort {
        fn query(
            &self,
            request: RepoMapQueryRequestV1,
        ) -> Result<RepoMapQueryResponseV1, CoreError> {
            Ok(RepoMapQueryResponseV1 {
                repo_id: request.repo_id,
                revision_id: request.revision_id,
                manifest_generation: request.manifest_generation,
                snapshot_meta: RepoMapSnapshotMetaV1 {
                    snapshot_id: "dispatch-snapshot".to_string(),
                    projection_version: 1,
                    authority_digest: "dispatch-digest".to_string(),
                    item_index_availability: "available".to_string(),
                    graph_coverage_class: "full".to_string(),
                    exactness_summary: "exact".to_string(),
                },
                entries: vec![RepoMapEntryDtoV1 {
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

    impl RepoMapBundleIngestPort for StubRepoMapIngestPort {
        fn ingest_bundle(&self, bundle: &RepoMapSourceBundleV1) -> Result<(), CoreError> {
            if bundle.file_indices.is_empty() {
                return Err(CoreError::InvalidContract(
                    "repo-map ingest: file_indices must not be empty".to_string(),
                ));
            }
            Ok(())
        }
    }

    impl RepoMapGenerationActivatePort for StubRepoMapActivatePort {
        fn activate_generation(
            &self,
            request: &RepoMapActivateGenerationRequestV1,
        ) -> Result<(), CoreError> {
            if request.manifest_digest.is_empty() {
                return Err(CoreError::InvalidContract(
                    "repo-map activate: manifest_digest must not be empty".to_string(),
                ));
            }
            Ok(())
        }
    }

    fn repo_map_request() -> RepoMapQueryRequestV1 {
        RepoMapQueryRequestV1 {
            repo_id: RepoId::new("repo-map-ipc"),
            revision_id: RevisionId::new("rev-map-ipc"),
            manifest_generation: ManifestGeneration::new(9),
            query_text: "dispatch owner".to_string(),
            top_k: 4,
            token_budget: 256,
            focus_subjects: vec![quanta_index_contract::RepoMapFocusSubjectDtoV1 {
                subject_identity: "src/lib.rs::Owner".to_string(),
                subject_doc_type: "Symbol".to_string(),
            }],
        }
    }

    fn into_repo_map_query_response(
        response: SearchPlaneIpcResponse,
    ) -> Result<RepoMapQueryResponseV1, Box<dyn std::error::Error>> {
        match response {
            SearchPlaneIpcResponse::RepoMapQuery(response) => Ok(response),
            other @ (SearchPlaneIpcResponse::Lexical(_)
            | SearchPlaneIpcResponse::Semantic(_)
            | SearchPlaneIpcResponse::Hybrid(_)
            | SearchPlaneIpcResponse::History(_)
            | SearchPlaneIpcResponse::Structural(_)
            | SearchPlaneIpcResponse::Bridge(_)
            | SearchPlaneIpcResponse::RepoMapMutationAck(_)
            | SearchPlaneIpcResponse::Explain(_)
            | SearchPlaneIpcResponse::Error(_)) => {
                Err(format!("expected repo-map query response, got {other:?}").into())
            }
        }
    }

    fn into_repo_map_mutation_ack(
        response: SearchPlaneIpcResponse,
    ) -> Result<RepoMapMutationAckV1, Box<dyn std::error::Error>> {
        match response {
            SearchPlaneIpcResponse::RepoMapMutationAck(ack) => Ok(ack),
            other @ (SearchPlaneIpcResponse::Lexical(_)
            | SearchPlaneIpcResponse::Semantic(_)
            | SearchPlaneIpcResponse::Hybrid(_)
            | SearchPlaneIpcResponse::History(_)
            | SearchPlaneIpcResponse::Structural(_)
            | SearchPlaneIpcResponse::Bridge(_)
            | SearchPlaneIpcResponse::RepoMapQuery(_)
            | SearchPlaneIpcResponse::Explain(_)
            | SearchPlaneIpcResponse::Error(_)) => {
                Err(format!("expected repo-map mutation ack, got {other:?}").into())
            }
        }
    }

    #[test]
    fn repo_map_dispatcher_branch_delegates_to_repo_map_query_port() -> TestResult {
        let dispatcher = SearchPlaneDispatcher::new(
            Arc::new(RejectLexicalOpener),
            Arc::new(RejectSemanticOpener),
            Arc::new(StubRepoMapIngestPort),
            Arc::new(StubRepoMapActivatePort),
            Arc::new(StubRepoMapQueryPort),
            Arc::new(RwLock::new(Ledger::default())),
        );

        let response = into_repo_map_query_response(
            dispatcher.dispatch(SearchPlaneIpcRequest::RepoMapQuery(repo_map_request())),
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
    fn repo_map_control_branches_ack_without_opening_other_indexes() -> TestResult {
        let dispatcher = SearchPlaneDispatcher::new(
            Arc::new(RejectLexicalOpener),
            Arc::new(RejectSemanticOpener),
            Arc::new(StubRepoMapIngestPort),
            Arc::new(StubRepoMapActivatePort),
            Arc::new(StubRepoMapQueryPort),
            Arc::new(RwLock::new(Ledger::default())),
        );

        let ingest = into_repo_map_mutation_ack(dispatcher.dispatch(
            SearchPlaneIpcRequest::RepoMapIngest(RepoMapSourceBundleV1 {
                repo_id: RepoId::new("repo-map-ipc"),
                revision_id: RevisionId::new("rev-map-ipc"),
                manifest_generation: ManifestGeneration::new(9),
                snapshot_id: "dispatch-snapshot".to_string(),
                projection_version: 1,
                authority_digest: "dispatch-digest".to_string(),
                item_index_availability: "available".to_string(),
                graph_coverage_class: "full".to_string(),
                exactness_summary: "exact".to_string(),
                redaction_state: "Unredacted".to_string(),
                file_indices: vec![quanta_index_contract::RepoMapFileIndexRecordV1 {
                    file_identity: "src/lib.rs".to_string(),
                    file_path: "src/lib.rs".to_string(),
                    file_kind: "library".to_string(),
                    line_count: 80,
                    symbol_records: vec![quanta_index_contract::RepoMapSymbolRecordDtoV1 {
                        subject_identity: "src/lib.rs::Owner".to_string(),
                        subject_doc_type: "Symbol".to_string(),
                        subject_kind: "service".to_string(),
                        symbol_name: "Owner".to_string(),
                        owner_path: "src/lib.rs".to_string(),
                    }],
                }],
                call_edges: vec![quanta_index_contract::RepoMapGraphEdgeDtoV1 {
                    from_identity: "src/lib.rs::Owner".to_string(),
                    to_identity: "src/lib.rs".to_string(),
                    edge_kind: "call".to_string(),
                }],
                import_edges: Vec::new(),
                chunk_records: vec![quanta_index_contract::RepoMapChunkRecordDtoV1 {
                    subject_identity: "src/lib.rs::Owner".to_string(),
                    owner_path: "src/lib.rs".to_string(),
                    token_count: 64,
                    preview_text: "dispatch owner symbol".to_string(),
                    exactness: "Exact".to_string(),
                }],
            }),
        ))?;
        if ingest.repo_id.as_str() != "repo-map-ipc" {
            return Err(format!("unexpected ingest repo id: {}", ingest.repo_id.as_str()).into());
        }
        if ingest.revision_id.as_str() != "rev-map-ipc" {
            return Err(format!(
                "unexpected ingest revision id: {}",
                ingest.revision_id.as_str()
            )
            .into());
        }
        if ingest.manifest_generation.get() != 9 {
            return Err(format!(
                "unexpected ingest manifest generation: {}",
                ingest.manifest_generation.get()
            )
            .into());
        }

        let activate = into_repo_map_mutation_ack(dispatcher.dispatch(
            SearchPlaneIpcRequest::RepoMapActivate(RepoMapActivateGenerationRequestV1 {
                repo_id: RepoId::new("repo-map-ipc"),
                revision_id: RevisionId::new("rev-map-ipc"),
                manifest_generation: ManifestGeneration::new(9),
                manifest_digest: "manifest-digest-9".to_string(),
            }),
        ))?;
        if activate.repo_id.as_str() != "repo-map-ipc" {
            return Err(
                format!("unexpected activate repo id: {}", activate.repo_id.as_str()).into(),
            );
        }
        if activate.revision_id.as_str() != "rev-map-ipc" {
            return Err(format!(
                "unexpected activate revision id: {}",
                activate.revision_id.as_str()
            )
            .into());
        }
        if activate.manifest_generation.get() != 9 {
            return Err(format!(
                "unexpected activate manifest generation: {}",
                activate.manifest_generation.get()
            )
            .into());
        }
        Ok(())
    }
}
