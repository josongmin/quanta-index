//! `QueryDispatcher` impl that routes IPC requests to lexical / semantic /
//! hybrid / explain handlers using the in-memory ledger as the readiness
//! source of truth.

use std::sync::{Arc, RwLock};

use quanta_index_contract::{
    GenerationPin, LqDirectiveSet, LqExpr, LqFilterSet, LqOptionSet, LqQuery, ManifestGeneration,
    RepoId, RevisionId, SearchExplanation, SearchPlaneExplainQueryRequest,
    SearchPlaneExplainQueryResponse, SearchPlaneHybridQueryRequest, SearchPlaneHybridQueryResponse,
    SearchPlaneIpcError, SearchPlaneIpcRequest, SearchPlaneIpcResponse,
    SearchPlaneLexicalQueryRequest, SearchPlaneLexicalQueryResponse, SearchPlaneSemanticQueryRequest,
    SearchPlaneSemanticQueryResponse,
};
use quanta_index_core::{
    CoreError, HybridOrchestratorPolicy, LexicalIndexOpenPort, LexicalPolicy,
    SemanticIndexOpenPort, SemanticPolicy,
};
use quanta_index_ipc::QueryDispatcher;
use quanta_index_lexical::LexicalAdapter;
use quanta_index_semantic::SemanticAdapter;

use crate::runtime::Ledger;

const ERR_INVALID: &str = "INVALID_REQUEST";
const ERR_NOT_READY: &str = "NOT_READY";
const ERR_NOT_IMPLEMENTED: &str = "NOT_IMPLEMENTED";
const ERR_INTERNAL: &str = "INTERNAL";

pub struct SearchPlaneDispatcher {
    lex_adapter: Arc<LexicalAdapter>,
    sem_adapter: Arc<SemanticAdapter>,
    ledger: Arc<RwLock<Ledger>>,
}

impl SearchPlaneDispatcher {
    #[must_use]
    pub fn new(
        lex_adapter: Arc<LexicalAdapter>,
        sem_adapter: Arc<SemanticAdapter>,
        ledger: Arc<RwLock<Ledger>>,
    ) -> Self {
        Self {
            lex_adapter,
            sem_adapter,
            ledger,
        }
    }

    fn lexical(
        &self,
        request: SearchPlaneLexicalQueryRequest,
    ) -> Result<SearchPlaneLexicalQueryResponse, CoreError> {
        LexicalPolicy::validate_query(&request.query)?;
        let pin = request.generation.ok_or_else(|| {
            CoreError::InvalidContract("lexical: generation pin required".to_string())
        })?;
        let materialized = self.snapshot_lex_seal()?;
        LexicalPolicy::validate_query_against_readiness(pin.manifest_generation, materialized)?;
        let searcher =
            self.lex_adapter
                .open(&pin.repo_id, &pin.revision_id, pin.manifest_generation)?;
        let results = searcher.search(&request.query, default_top_k())?;
        Ok(SearchPlaneLexicalQueryResponse {
            generation: pin,
            results,
        })
    }

    fn semantic(
        &self,
        request: SearchPlaneSemanticQueryRequest,
    ) -> Result<SearchPlaneSemanticQueryResponse, CoreError> {
        SemanticPolicy::validate_top_k(request.top_k)?;
        let pin = request.generation.ok_or_else(|| {
            CoreError::InvalidContract("semantic: generation pin required".to_string())
        })?;
        let materialized = self.snapshot_sem_seal()?;
        SemanticPolicy::validate_query_against_readiness(pin.manifest_generation, materialized)?;
        // Reference adapter does NOT embed query text. The query_text is
        // treated as a raw byte source for the vector by f32 LE decoding,
        // which is sufficient for a deterministic integration test.
        let query_vector = encode_query_text_as_vector(&request.query_text);
        let searcher =
            self.sem_adapter
                .open(&pin.repo_id, &pin.revision_id, pin.manifest_generation)?;
        let results = searcher.search(&query_vector, request.top_k)?;
        Ok(SearchPlaneSemanticQueryResponse {
            generation: pin,
            results,
        })
    }

    fn hybrid(
        &self,
        request: SearchPlaneHybridQueryRequest,
    ) -> Result<SearchPlaneHybridQueryResponse, CoreError> {
        let pin = request.generation.ok_or_else(|| {
            CoreError::InvalidContract("hybrid: generation pin required".to_string())
        })?;
        let lex_seal = self.snapshot_lex_seal()?;
        let sem_seal = self.snapshot_sem_seal()?;
        HybridOrchestratorPolicy::validate_joint_readiness(
            pin.manifest_generation,
            lex_seal,
            sem_seal,
        )?;

        let lex_searcher =
            self.lex_adapter
                .open(&pin.repo_id, &pin.revision_id, pin.manifest_generation)?;
        let sem_searcher =
            self.sem_adapter
                .open(&pin.repo_id, &pin.revision_id, pin.manifest_generation)?;
        let lex_results = lex_searcher.search(&request.lexical_query, request.top_k)?;
        let query_vector = encode_query_text_as_vector(&request.semantic_query_text);
        let sem_results = sem_searcher.search(&query_vector, request.top_k)?;
        let fused = HybridOrchestratorPolicy::fuse_rrf(&lex_results, &sem_results, request.top_k);
        Ok(SearchPlaneHybridQueryResponse {
            generation: pin,
            results: fused,
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
            self.lex_adapter
                .open(&pin.repo_id, &pin.revision_id, pin.manifest_generation)?;
        let probe_text = if request.candidate.snippet.is_empty() {
            request.candidate.candidate_id.clone()
        } else {
            request.candidate.snippet.clone()
        };
        let probe = LqQuery {
            expr: LqExpr::Raw(probe_text),
            filters: LqFilterSet::default(),
            options: LqOptionSet::default(),
            directives: LqDirectiveSet::default(),
        };
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
            explanation: SearchExplanation { summary },
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

impl QueryDispatcher for SearchPlaneDispatcher {
    fn dispatch(&self, request: SearchPlaneIpcRequest) -> SearchPlaneIpcResponse {
        match request {
            SearchPlaneIpcRequest::Lexical(req) => match self.lexical(req) {
                Ok(resp) => SearchPlaneIpcResponse::Lexical(resp),
                Err(err) => SearchPlaneIpcResponse::Error(core_error_to_ipc(err)),
            },
            SearchPlaneIpcRequest::Semantic(req) => match self.semantic(req) {
                Ok(resp) => SearchPlaneIpcResponse::Semantic(resp),
                Err(err) => SearchPlaneIpcResponse::Error(core_error_to_ipc(err)),
            },
            SearchPlaneIpcRequest::Hybrid(req) => match self.hybrid(req) {
                Ok(resp) => SearchPlaneIpcResponse::Hybrid(resp),
                Err(err) => SearchPlaneIpcResponse::Error(core_error_to_ipc(err)),
            },
            SearchPlaneIpcRequest::Explain(req) => match self.explain(req) {
                Ok(resp) => SearchPlaneIpcResponse::Explain(resp),
                Err(err) => SearchPlaneIpcResponse::Error(core_error_to_ipc(err)),
            },
        }
    }
}

fn core_error_to_ipc(err: CoreError) -> SearchPlaneIpcError {
    let (code, message) = match err {
        CoreError::InvalidContract(msg) => (ERR_INVALID, msg),
        CoreError::NotReady(msg) => (ERR_NOT_READY, msg),
        CoreError::NotImplemented(msg) => (ERR_NOT_IMPLEMENTED, msg),
        CoreError::NotFound(msg) => (ERR_NOT_READY, msg),
        CoreError::Storage(msg) => (ERR_INTERNAL, msg),
    };
    SearchPlaneIpcError {
        code: code.to_string(),
        message,
    }
}

const fn default_top_k() -> u32 {
    50
}

/// For the reference semantic adapter, the query text is interpreted as a
/// space-separated decimal list of f32 values. Empty or unparseable input
/// yields an empty vector. This is enough for deterministic e2e tests.
fn encode_query_text_as_vector(text: &str) -> Vec<f32> {
    text.split_whitespace()
        .filter_map(|tok| tok.parse::<f32>().ok())
        .collect()
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

