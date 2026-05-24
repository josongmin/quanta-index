//! `QueryDispatcher` impl that routes IPC requests to lexical / semantic /
//! hybrid / explain handlers using the in-memory ledger as the readiness
//! source of truth.

use std::sync::{Arc, RwLock};

use quanta_index_contract::{
    GenerationPin, LqDirectiveSet, LqExpr, LqFilterSet, LqOptionSet, LqQuery, ManifestGeneration,
    RepoId, RepoMapActivateGenerationRequestV1, RepoMapMutationAckV1, RepoMapQueryRequestV1,
    RepoMapQueryResponseV1, RepoMapSourceBundleV1, RevisionId, SearchExplanation,
    SearchPlaneExplainQueryRequest, SearchPlaneExplainQueryResponse, SearchPlaneHybridQueryRequest,
    SearchPlaneHybridQueryResponse, SearchPlaneIpcError, SearchPlaneIpcRequest,
    SearchPlaneIpcResponse, SearchPlaneLexicalQueryRequest, SearchPlaneLexicalQueryResponse,
    SearchPlaneSemanticQueryRequest, SearchPlaneSemanticQueryResponse,
};
use quanta_index_core::{
    CoreError, ExplainQueryPort, HybridOrchestratorPolicy, HybridQueryPort, LexicalIndexOpenPort,
    LexicalPolicy, LexicalQueryPort, RepoMapBundleIngestPort, RepoMapGenerationActivatePort,
    RepoMapPolicy, RepoMapQueryPort, SemanticIndexOpenPort, SemanticPolicy, SemanticQueryPort,
};
use quanta_index_ipc::QueryDispatcher;

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
        request: SearchPlaneLexicalQueryRequest,
    ) -> Result<SearchPlaneLexicalQueryResponse, CoreError> {
        LexicalPolicy::validate_query(&request.query)?;
        let pin = request.generation.ok_or_else(|| {
            CoreError::InvalidContract("lexical: generation pin required".to_string())
        })?;
        let materialized = self.snapshot_lex_seal()?;
        LexicalPolicy::validate_query_against_readiness(pin.manifest_generation, materialized)?;
        let searcher =
            self.lex_opener
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
            self.sem_opener
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
            self.lex_opener
                .open(&pin.repo_id, &pin.revision_id, pin.manifest_generation)?;
        let sem_searcher =
            self.sem_opener
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
            self.lex_opener
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
        request: SearchPlaneLexicalQueryRequest,
    ) -> Result<SearchPlaneLexicalQueryResponse, CoreError> {
        self.lexical(request)
    }
}

impl SemanticQueryPort for SearchPlaneDispatcher {
    fn semantic_query(
        &self,
        request: SearchPlaneSemanticQueryRequest,
    ) -> Result<SearchPlaneSemanticQueryResponse, CoreError> {
        self.semantic(request)
    }
}

impl HybridQueryPort for SearchPlaneDispatcher {
    fn hybrid_query(
        &self,
        request: SearchPlaneHybridQueryRequest,
    ) -> Result<SearchPlaneHybridQueryResponse, CoreError> {
        self.hybrid(request)
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
        CoreError::InvalidContract(msg) => (ERR_INVALID, msg),
        CoreError::NotReady(msg) => (ERR_NOT_READY, msg),
        CoreError::NotImplemented(msg) => (ERR_NOT_IMPLEMENTED, msg),
        CoreError::NotFound(msg) => (ERR_NOT_FOUND, msg),
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

#[cfg(test)]
mod tests {
    use std::sync::{Arc, RwLock};

    use quanta_index_contract::{
        ManifestGeneration, RepoId, RepoMapActivateGenerationRequestV1, RepoMapEntryDtoV1,
        RepoMapMutationAckV1, RepoMapQueryRequestV1, RepoMapQueryResponseV1,
        RepoMapSnapshotMetaV1, RepoMapSourceBundleV1, RevisionId, SearchPlaneIpcRequest,
        SearchPlaneIpcResponse,
    };
    use quanta_index_core::{
        CoreError, LexicalIndexOpenPort, LexicalSearcher, RepoMapBundleIngestPort,
        RepoMapGenerationActivatePort, RepoMapQueryPort, SemanticIndexOpenPort, SemanticSearcher,
    };
    use quanta_index_ipc::QueryDispatcher;

    use super::SearchPlaneDispatcher;
    use crate::runtime::Ledger;

    struct PanicLexicalOpener;

    impl LexicalIndexOpenPort for PanicLexicalOpener {
        fn open(
            &self,
            _repo: &RepoId,
            _revision: &RevisionId,
            _generation: ManifestGeneration,
        ) -> Result<Box<dyn LexicalSearcher>, CoreError> {
            panic!("repo-map dispatch should not open lexical index")
        }
    }

    struct PanicSemanticOpener;

    impl SemanticIndexOpenPort for PanicSemanticOpener {
        fn open(
            &self,
            _repo: &RepoId,
            _revision: &RevisionId,
            _generation: ManifestGeneration,
        ) -> Result<Box<dyn SemanticSearcher>, CoreError> {
            panic!("repo-map dispatch should not open semantic index")
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
                degraded_reason_codes: vec!["external_query_bootstrap".to_string()],
            })
        }
    }

    impl RepoMapBundleIngestPort for StubRepoMapIngestPort {
        fn ingest_bundle(&self, bundle: &RepoMapSourceBundleV1) -> Result<(), CoreError> {
            if bundle.entry_identities.is_empty() {
                return Err(CoreError::InvalidContract(
                    "repo-map ingest: entry_identities must not be empty".to_string(),
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

    #[test]
    fn repo_map_dispatcher_branch_delegates_to_repo_map_query_port() {
        let dispatcher = SearchPlaneDispatcher::new(
            Arc::new(PanicLexicalOpener),
            Arc::new(PanicSemanticOpener),
            Arc::new(StubRepoMapIngestPort),
            Arc::new(StubRepoMapActivatePort),
            Arc::new(StubRepoMapQueryPort),
            Arc::new(RwLock::new(Ledger::default())),
        );

        let response = dispatcher.dispatch(SearchPlaneIpcRequest::RepoMapQuery(repo_map_request()));

        match response {
            SearchPlaneIpcResponse::RepoMapQuery(response) => {
                assert_eq!(response.repo_id.as_str(), "repo-map-ipc");
                assert_eq!(response.revision_id.as_str(), "rev-map-ipc");
                assert_eq!(response.manifest_generation.get(), 9);
                assert_eq!(response.snapshot_meta.snapshot_id, "dispatch-snapshot");
                assert_eq!(response.entries.len(), 1);
                assert_eq!(response.entries[0].owner_path, "src/lib.rs");
            }
            other => panic!("expected repo-map query response, got {other:?}"),
        }
    }

    #[test]
    fn repo_map_control_branches_ack_without_opening_other_indexes() {
        let dispatcher = SearchPlaneDispatcher::new(
            Arc::new(PanicLexicalOpener),
            Arc::new(PanicSemanticOpener),
            Arc::new(StubRepoMapIngestPort),
            Arc::new(StubRepoMapActivatePort),
            Arc::new(StubRepoMapQueryPort),
            Arc::new(RwLock::new(Ledger::default())),
        );

        let ingest = dispatcher.dispatch(SearchPlaneIpcRequest::RepoMapIngest(RepoMapSourceBundleV1 {
            repo_id: RepoId::new("repo-map-ipc"),
            revision_id: RevisionId::new("rev-map-ipc"),
            manifest_generation: ManifestGeneration::new(9),
            snapshot_id: "dispatch-snapshot".to_string(),
            projection_version: 1,
            authority_digest: "dispatch-digest".to_string(),
            item_index_availability: "available".to_string(),
            graph_coverage_class: "full".to_string(),
            exactness_summary: "exact".to_string(),
            entry_identities: vec!["src/lib.rs::Owner".to_string()],
        }));
        match ingest {
            SearchPlaneIpcResponse::RepoMapMutationAck(RepoMapMutationAckV1 {
                repo_id,
                revision_id,
                manifest_generation,
            }) => {
                assert_eq!(repo_id.as_str(), "repo-map-ipc");
                assert_eq!(revision_id.as_str(), "rev-map-ipc");
                assert_eq!(manifest_generation.get(), 9);
            }
            other => panic!("expected repo-map mutation ack, got {other:?}"),
        }

        let activate = dispatcher.dispatch(SearchPlaneIpcRequest::RepoMapActivate(
            RepoMapActivateGenerationRequestV1 {
                repo_id: RepoId::new("repo-map-ipc"),
                revision_id: RevisionId::new("rev-map-ipc"),
                manifest_generation: ManifestGeneration::new(9),
                manifest_digest: "manifest-digest-9".to_string(),
            },
        ));
        match activate {
            SearchPlaneIpcResponse::RepoMapMutationAck(RepoMapMutationAckV1 {
                repo_id,
                revision_id,
                manifest_generation,
            }) => {
                assert_eq!(repo_id.as_str(), "repo-map-ipc");
                assert_eq!(revision_id.as_str(), "rev-map-ipc");
                assert_eq!(manifest_generation.get(), 9);
            }
            other => panic!("expected repo-map mutation ack, got {other:?}"),
        }
    }
}
