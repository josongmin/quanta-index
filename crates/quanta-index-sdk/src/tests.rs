use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use quanta_index_contract::lex::{
    CommitRecord, CommitSha, DiffHunkRecord, DirtyRecord, LanguageCode, ParseNode, ParseRoleTag,
    ParseTreeRecord, SymbolKindCode, SymbolKindFamily, SymbolRecord, SymbolRelationship,
    SymbolSpan, compute_parse_tree_source_hash,
};
use quanta_index_contract::{
    BatchPublishReceipt, ChunkId, ChunkRecord, DiffHunkSide, GenerationSelector,
    HistoryQueryRequest, HybridSeedCandidate, HybridSeedLane, HybridSeedQueryResponse,
    ManifestGeneration, PlannerStage, PlannerTraceEntry, RepoId, RepoMapChunkExactness,
    RepoMapExactnessSummary, RepoMapGraphCoverageClass, RepoMapItemIndexAvailability,
    RepoMapMutationAck, RepoMapRedactionState, RepoRelativePath, RevisionId,
    RuntimeMetadataQueryRequest, SearchExplanation, SearchPlaneActivationAck,
    SearchPlaneControlIpcRequestEnvelope, SearchPlaneControlIpcResponseEnvelope,
    SearchPlaneHistoryQueryResponse, SearchPlaneIngestIpcRequest,
    SearchPlaneIngestIpcRequestEnvelope, SearchPlaneIngestIpcResponse,
    SearchPlaneIngestIpcResponseEnvelope, SearchPlaneQueryIpcRequestEnvelope,
    SearchPlaneQueryIpcResponse, SearchPlaneQueryIpcResponseEnvelope,
    SearchPlaneRuntimeMetadataQueryResponse, SearchPlaneStructuralQueryResponse, SearchScopeKey,
    SearchScopeSurface, SemanticQueryResponse, StructuralQueryRequest, SymbolId, TextQueryResponse,
};

use crate::{
    ConnectOptions, ControlTransport, DirtyBatch, HistoryBatch, IngestTransport, QuantaIndex,
    QueryTransport, SearchCorpusBatch, StructuralBatch, Track,
};

/// QI-SDK-01: small helper to unwrap a `Result` inside a `#[test]` with
/// a clear panic message. Replaces an earlier `assert!(false, ...)` +
/// `return;` macro that tripped `clippy::assertions_on_constants`.
macro_rules! ok_or_fail {
    ($expr:expr $(,)?) => {
        match $expr {
            Ok(value) => value,
            Err(err) => panic!("unexpected error: {err}"),
        }
    };
}

struct StubQueryTransport {
    requests: Mutex<Vec<SearchPlaneQueryIpcRequestEnvelope>>,
    response: Mutex<Option<SearchPlaneQueryIpcResponse>>,
}

impl StubQueryTransport {
    fn new(response: SearchPlaneQueryIpcResponse) -> Self {
        Self {
            requests: Mutex::new(Vec::new()),
            response: Mutex::new(Some(response)),
        }
    }
}

impl QueryTransport for StubQueryTransport {
    fn send(
        &self,
        request: SearchPlaneQueryIpcRequestEnvelope,
    ) -> Result<SearchPlaneQueryIpcResponseEnvelope, crate::SdkError> {
        self.requests
            .lock()
            .map_err(|err| crate::SdkError::Protocol(format!("query transport poisoned: {err}")))?
            .push(request.clone());
        let payload = self
            .response
            .lock()
            .map_err(|err| crate::SdkError::Protocol(format!("query response poisoned: {err}")))?
            .take()
            .ok_or_else(|| crate::SdkError::Protocol("missing stub query response".to_string()))?;
        Ok(SearchPlaneQueryIpcResponseEnvelope {
            request_id: request.request_id,
            payload,
        })
    }
}

struct StubControlTransport {
    requests: Mutex<Vec<SearchPlaneControlIpcRequestEnvelope>>,
    response: Mutex<Option<quanta_index_contract::SearchPlaneControlIpcResponse>>,
}

impl StubControlTransport {
    fn new(response: quanta_index_contract::SearchPlaneControlIpcResponse) -> Self {
        Self {
            requests: Mutex::new(Vec::new()),
            response: Mutex::new(Some(response)),
        }
    }
}

impl ControlTransport for StubControlTransport {
    fn send(
        &self,
        request: SearchPlaneControlIpcRequestEnvelope,
    ) -> Result<SearchPlaneControlIpcResponseEnvelope, crate::SdkError> {
        self.requests
            .lock()
            .map_err(|err| crate::SdkError::Protocol(format!("control transport poisoned: {err}")))?
            .push(request.clone());
        let payload = self
            .response
            .lock()
            .map_err(|err| crate::SdkError::Protocol(format!("control response poisoned: {err}")))?
            .take()
            .ok_or_else(|| {
                crate::SdkError::Protocol("missing stub control response".to_string())
            })?;
        Ok(SearchPlaneControlIpcResponseEnvelope {
            request_id: request.request_id,
            payload,
        })
    }
}

/// QI-SDK-01: stub ingest transport. Replaces the old channel-publisher
/// fixtures the SDK used to spin up. Records incoming requests so tests can
/// assert on the typed batch the SDK assembled.
struct StubIngestTransport {
    requests: Mutex<Vec<SearchPlaneIngestIpcRequestEnvelope>>,
    response: Mutex<Option<SearchPlaneIngestIpcResponse>>,
}

impl StubIngestTransport {
    fn new(response: SearchPlaneIngestIpcResponse) -> Self {
        Self {
            requests: Mutex::new(Vec::new()),
            response: Mutex::new(Some(response)),
        }
    }
}

impl IngestTransport for StubIngestTransport {
    fn send(
        &self,
        request: SearchPlaneIngestIpcRequestEnvelope,
    ) -> Result<SearchPlaneIngestIpcResponseEnvelope, crate::SdkError> {
        self.requests
            .lock()
            .map_err(|err| crate::SdkError::Protocol(format!("ingest transport poisoned: {err}")))?
            .push(request.clone());
        let payload = self
            .response
            .lock()
            .map_err(|err| crate::SdkError::Protocol(format!("ingest response poisoned: {err}")))?
            .take()
            .ok_or_else(|| crate::SdkError::Protocol("missing stub ingest response".to_string()))?;
        Ok(SearchPlaneIngestIpcResponseEnvelope {
            request_id: request.request_id,
            payload,
        })
    }
}

fn sample_generation_pin() -> quanta_index_contract::GenerationPin {
    quanta_index_contract::GenerationPin::new(repo_id(), revision_id(), ManifestGeneration::new(7))
}

fn repo_id() -> RepoId {
    RepoId::new("repo-1")
}

fn revision_id() -> RevisionId {
    RevisionId::new("rev-1")
}

fn sample_hit() -> quanta_index_contract::LexicalCandidate {
    quanta_index_contract::LexicalCandidate {
        candidate_id: "chunk-1".to_string(),
        repo_id: repo_id(),
        revision_id: revision_id(),
        manifest_generation: ManifestGeneration::new(7),
        repo_relative_path: RepoRelativePath::new("src/lib.rs"),
        start_line: 10,
        end_line: 20,
        score: 1.0,
        snippet: "fn sample() {}".to_string(),
        snippet_hit_offset: None,
        highlights: Vec::new(),
    }
}

fn sample_hybrid_seed_candidate() -> HybridSeedCandidate {
    HybridSeedCandidate {
        candidate: sample_hit(),
        seed_rank: 1,
        lexical_rank: Some(1),
        lexical_score_raw: Some(1.0),
        semantic_rank: Some(2),
        semantic_score_raw: Some(0.5),
        source_lanes: vec![HybridSeedLane::Lexical, HybridSeedLane::Semantic],
    }
}

fn sample_symbol_hit() -> quanta_index_contract::SymbolCandidate {
    quanta_index_contract::SymbolCandidate {
        candidate_id: "sym-1".to_string(),
        repo_id: repo_id(),
        revision_id: revision_id(),
        manifest_generation: ManifestGeneration::new(7),
        repo_relative_path: RepoRelativePath::new("src/lib.rs"),
        start_line: 1,
        end_line: 1,
        score: 1.0,
        snippet: "sample crate".to_string(),
        symbol_kind: ok_or_fail!(SymbolKindCode::new("function")),
        symbol_kind_family: Some(SymbolKindFamily::Callable),
    }
}

fn sample_explanation() -> SearchExplanation {
    SearchExplanation {
        planner_trace: vec![PlannerTraceEntry {
            stage: PlannerStage::Plan,
            detail: "planned".to_string(),
        }],
        engines_touched: vec![quanta_index_contract::EngineTouched::Semantic],
        early_stop_reason: None,
        contributions: Vec::new(),
        ranker_weights_hash: [0; 32],
        strategy: "test".to_string(),
        summary: "ok".to_string(),
    }
}

fn sample_symbol() -> SymbolRecord {
    SymbolRecord {
        symbol_id: SymbolId::new("sym-1"),
        repo_relative_path: RepoRelativePath::new("src/lib.rs"),
        language: ok_or_fail!(LanguageCode::new("rust")),
        symbol_kind: ok_or_fail!(SymbolKindCode::new("function")),
        symbol_kind_family: Some(SymbolKindFamily::Callable),
        local_name: "sample".into(),
        qualified_name: "crate::sample".into(),
        signature: Some("fn sample()".into()),
        visibility: None,
        definition_span: SymbolSpan {
            path: "src/lib.rs".into(),
            byte_start: 0,
            byte_end: 10,
            line_start: 1,
            line_end: 1,
        },
        container_qualified_name: None,
        relationship: SymbolRelationship::Def,
    }
}

fn sample_chunk() -> ChunkRecord {
    ChunkRecord {
        chunk_id: ChunkId::new("chunk-1"),
        repo_relative_path: RepoRelativePath::new("src/lib.rs"),
        language: ok_or_fail!(LanguageCode::new("rust")),
        start_byte: 0,
        end_byte: 16,
        start_line: 1,
        end_line: 4,
        text: "fn sample() {}".into(),
        structural: None,
        parent_chunk_id: None,
        source_repo_id: None,
    }
}

fn sample_search_scope() -> SearchScopeKey {
    SearchScopeKey {
        doc_surface: SearchScopeSurface::File,
        repo_relative_path: RepoRelativePath::new("src/lib.rs"),
    }
}

fn sample_repomap_focus_subject() -> quanta_index_contract::RepoMapFocusSubjectDto {
    quanta_index_contract::RepoMapFocusSubjectDto {
        subject_identity: "subject://repomap".to_string(),
        subject_doc_type: quanta_index_contract::RepoMapDocType::Symbol,
    }
}

fn sample_repomap_snapshot_meta() -> quanta_index_contract::RepoMapSnapshotMeta {
    quanta_index_contract::RepoMapSnapshotMeta {
        snapshot_id: "snap-1".to_string(),
        projection_version: 7,
        authority_digest: "blake3:deadbeef".to_string(),
        item_index_availability: RepoMapItemIndexAvailability::Full,
        graph_coverage_class: RepoMapGraphCoverageClass::Complete,
        exactness_summary: RepoMapExactnessSummary::Exact,
    }
}

fn sample_repomap_entry() -> quanta_index_contract::RepoMapEntryDto {
    quanta_index_contract::RepoMapEntryDto {
        subject_identity: "entry::ident".to_string(),
        subject_doc_type: quanta_index_contract::RepoMapDocType::Symbol,
        subject_kind: "function".to_string(),
        owner_path: "src/lib.rs".to_string(),
        score: 0.875,
        final_score_millis: 875,
        included: true,
        rank: 1,
        importance_score_millis: 500,
        utility_score_millis: 400,
        freshness_score_millis: 300,
        evidence_priority_millis: 200,
        token_budget_hint: 1024,
        contributing_signals: std::collections::BTreeMap::from([
            ("centrality".to_string(), 100_i64),
            ("recency".to_string(), -3_i64),
        ]),
        projection_evidence_kind: "authoritative".to_string(),
        projection_authority_artifact_id: "art-1".to_string(),
        projection_authority_digest: "blake3:cafebabe".to_string(),
        projection_status: "ok".to_string(),
        redaction_state: RepoMapRedactionState::Unredacted,
    }
}

fn sample_repomap_query_request() -> quanta_index_contract::RepoMapQueryRequest {
    quanta_index_contract::RepoMapQueryRequest {
        repo_id: repo_id(),
        revision_id: revision_id(),
        manifest_generation: ManifestGeneration::new(7),
        query_text: "repo map focus".to_string(),
        top_k: 5,
        token_budget: 2048,
        focus_subjects: vec![sample_repomap_focus_subject()],
    }
}

fn sample_repomap_query_response() -> quanta_index_contract::RepoMapQueryResponse {
    quanta_index_contract::RepoMapQueryResponse {
        repo_id: repo_id(),
        revision_id: revision_id(),
        manifest_generation: ManifestGeneration::new(7),
        snapshot_meta: sample_repomap_snapshot_meta(),
        entries: vec![sample_repomap_entry()],
        dropped_entries_count: 1,
        drop_reason_codes: vec!["token_budget".to_string()],
        degraded_reason_codes: vec!["partial_authority".to_string()],
    }
}

fn sample_commit_sha() -> CommitSha {
    CommitSha::from_bytes([
        0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd,
        0xef, 0x01, 0x23, 0x45, 0x67,
    ])
}

fn sample_commit_record() -> CommitRecord {
    CommitRecord {
        wire_version: 1,
        sha: sample_commit_sha(),
        parents: vec![],
        author_time_ms: 11,
        committer_time_ms: 12,
        applied_at_ms: 13,
        author: "alice".into(),
        author_name: None,
        author_email: None,
        committer: "alice".into(),
        committer_name: None,
        committer_email: None,
        message: "fix: sample".into(),
        is_merge: false,
        tags: vec!["v1.0.0".into()],
    }
}

fn sample_diff_record() -> DiffHunkRecord {
    DiffHunkRecord {
        wire_version: 1,
        hunk_header: "@@ -1,1 +1,2 @@".into(),
        side: DiffHunkSide::After,
        added_text: "todo!".into(),
        removed_text: "".into(),
        touched_text: "todo!".into(),
        byte_start: 0,
        byte_end: 5,
    }
}

fn sample_dirty_record() -> DirtyRecord {
    DirtyRecord {
        wire_version: 1,
        doc_id: ChunkId::new("chunk-dirty"),
        applied_at_ms: 55,
        payload_hash: [7; 32],
    }
}

fn sample_parse_tree_record() -> ParseTreeRecord {
    ParseTreeRecord {
        wire_version: 1,
        lang: ok_or_fail!(LanguageCode::new("rust")),
        root: ParseNode {
            kind: "function_item".into(),
            byte_start: 0,
            byte_end: 10,
            children: vec![],
        },
        source_hash: compute_parse_tree_source_hash("fn sample() {}"),
        role_tag_schema_version: 1,
        role_tags: vec![ParseRoleTag {
            role: "expr".into(),
            byte_start: 0,
            byte_end: 4,
        }],
    }
}

fn unused_query() -> Arc<StubQueryTransport> {
    Arc::new(StubQueryTransport::new(SearchPlaneQueryIpcResponse::Text(
        TextQueryResponse {
            generation: sample_generation_pin(),
            results: vec![],
            file_owner_rows: None,
        },
    )))
}

fn unused_control() -> Arc<StubControlTransport> {
    Arc::new(StubControlTransport::new(
        quanta_index_contract::SearchPlaneControlIpcResponse::ActivationAck(
            SearchPlaneActivationAck {
                repo_id: repo_id(),
                revision_id: revision_id(),
                manifest_generation: ManifestGeneration::new(7),
                manifest_digest: "digest".to_string(),
                tracks: vec![Track::Lexical],
            },
        ),
    ))
}

fn unused_ingest() -> Arc<StubIngestTransport> {
    Arc::new(StubIngestTransport::new(
        SearchPlaneIngestIpcResponse::SearchCorpusReceipt(BatchPublishReceipt::default()),
    ))
}

fn only_query_request(
    transport: &StubQueryTransport,
) -> Result<SearchPlaneQueryIpcRequestEnvelope, crate::SdkError> {
    let requests = transport
        .requests
        .lock()
        .map_err(|err| crate::SdkError::Protocol(format!("query request list poisoned: {err}")))?;
    let len = requests.len();
    if len != 1 {
        return Err(crate::SdkError::Protocol(format!(
            "expected exactly one query request, got {len}"
        )));
    }
    requests
        .first()
        .cloned()
        .ok_or_else(|| crate::SdkError::Protocol("missing captured query request".to_string()))
}

fn only_control_request(
    transport: &StubControlTransport,
) -> Result<SearchPlaneControlIpcRequestEnvelope, crate::SdkError> {
    let requests = transport.requests.lock().map_err(|err| {
        crate::SdkError::Protocol(format!("control request list poisoned: {err}"))
    })?;
    let len = requests.len();
    if len != 1 {
        return Err(crate::SdkError::Protocol(format!(
            "expected exactly one control request, got {len}"
        )));
    }
    requests
        .first()
        .cloned()
        .ok_or_else(|| crate::SdkError::Protocol("missing captured control request".to_string()))
}

fn only_ingest_request(
    transport: &StubIngestTransport,
) -> Result<SearchPlaneIngestIpcRequestEnvelope, crate::SdkError> {
    let requests = transport
        .requests
        .lock()
        .map_err(|err| crate::SdkError::Protocol(format!("ingest request list poisoned: {err}")))?;
    let len = requests.len();
    if len != 1 {
        return Err(crate::SdkError::Protocol(format!(
            "expected exactly one ingest request, got {len}"
        )));
    }
    requests
        .first()
        .cloned()
        .ok_or_else(|| crate::SdkError::Protocol("missing captured ingest request".to_string()))
}

#[test]
fn connect_options_from_state_root_resolve_default_sockets() {
    let resolved = ok_or_fail!(ConnectOptions::from_state_root("/tmp/qi-state").resolve());
    assert_eq!(resolved.0, Some(PathBuf::from("/tmp/qi-state")));
    assert_eq!(
        resolved.1,
        PathBuf::from("/tmp/qi-state/search-plane/query.sock")
    );
    assert_eq!(
        resolved.2,
        PathBuf::from("/tmp/qi-state/search-plane/control.sock")
    );
    assert_eq!(
        resolved.3,
        PathBuf::from("/tmp/qi-state/search-plane/ingest.sock"),
        "QI-SDK-01: ingest socket resolves to state_root/search-plane/ingest.sock"
    );
}

#[test]
fn semantic_query_builder_emits_active_selector_and_query_text() {
    let query = Arc::new(StubQueryTransport::new(
        SearchPlaneQueryIpcResponse::Semantic(SemanticQueryResponse {
            generation: sample_generation_pin(),
            results: vec![sample_hit()],
            explanation: sample_explanation(),
        }),
    ));
    let control = unused_control();
    let ingest = unused_ingest();
    let client = QuantaIndex::from_transports(query.clone(), control, ingest);
    let response = client
        .semantic()
        .query()
        .active(repo_id(), revision_id())
        .text("0.1 0.2 0.3")
        .top_k(5)
        .execute();
    let _response = ok_or_fail!(response);
    let captured = ok_or_fail!(only_query_request(query.as_ref()));
    assert!(
        matches!(
            &captured.payload,
            quanta_index_contract::SearchPlaneQueryIpcRequest::Semantic(_)
        ),
        "expected semantic request, got {:?}",
        captured.payload
    );
    let quanta_index_contract::SearchPlaneQueryIpcRequest::Semantic(req) = &captured.payload else {
        return;
    };
    assert_eq!(req.top_k, 5);
    assert_eq!(req.query_text.as_str(), "0.1 0.2 0.3");
    assert!(matches!(
        req.generation_selector,
        Some(GenerationSelector::Active { .. })
    ));
}

#[test]
fn semantic_scope_sourcegraph_query_preserves_scope_wire_fields() {
    let query = Arc::new(StubQueryTransport::new(
        SearchPlaneQueryIpcResponse::Semantic(SemanticQueryResponse {
            generation: sample_generation_pin(),
            results: vec![sample_hit()],
            explanation: sample_explanation(),
        }),
    ));
    let client = QuantaIndex::from_transports(query.clone(), unused_control(), unused_ingest());
    let _response = ok_or_fail!(
        client
            .semantic()
            .query()
            .active(repo_id(), revision_id())
            .text("1.0 0.0")
            .scope_sourcegraph("repo:repo-1 file:lib.rs")
            .scope_top_k(8)
            .top_k(5)
            .execute()
    );
    let captured = ok_or_fail!(only_query_request(query.as_ref()));
    assert!(
        matches!(
            &captured.payload,
            quanta_index_contract::SearchPlaneQueryIpcRequest::Semantic(_)
        ),
        "expected semantic request, got {:?}",
        captured.payload
    );
    let quanta_index_contract::SearchPlaneQueryIpcRequest::Semantic(req) = &captured.payload else {
        return;
    };
    assert_eq!(req.top_k, 5);
    assert!(
        req.lexical_scope.is_some(),
        "expected semantic lexical scope"
    );
    let Some(scope) = &req.lexical_scope else {
        return;
    };
    assert_eq!(
        scope.syntax,
        quanta_index_contract::TextQuerySyntax::Sourcegraph
    );
    assert_eq!(scope.query_text, "repo:repo-1 file:lib.rs");
    assert_eq!(scope.top_k, 8);
}

#[test]
fn lexical_query_builder_carries_top_k_to_wire_contract() {
    let query = Arc::new(StubQueryTransport::new(SearchPlaneQueryIpcResponse::Text(
        TextQueryResponse {
            generation: sample_generation_pin(),
            results: vec![sample_hit()],
            file_owner_rows: None,
        },
    )));
    let client = QuantaIndex::from_transports(query.clone(), unused_control(), unused_ingest());
    let _response = ok_or_fail!(
        client
            .lexical()
            .query()
            .native("needle")
            .active(repo_id(), revision_id())
            .top_k(42)
            .execute()
    );
    let captured = ok_or_fail!(only_query_request(query.as_ref()));
    assert!(
        matches!(
            &captured.payload,
            quanta_index_contract::SearchPlaneQueryIpcRequest::Text(_)
        ),
        "expected text request, got {:?}",
        captured.payload
    );
    let quanta_index_contract::SearchPlaneQueryIpcRequest::Text(req) = &captured.payload else {
        return;
    };
    assert_eq!(
        req.top_k, 42,
        "QI-QRY-01: TextQueryRequest.top_k must be set from builder"
    );
}

#[test]
fn lexical_query_request_forwards_contract_dto_unchanged() {
    let query = Arc::new(StubQueryTransport::new(SearchPlaneQueryIpcResponse::Text(
        TextQueryResponse {
            generation: sample_generation_pin(),
            results: vec![sample_hit()],
            file_owner_rows: None,
        },
    )));
    let client = QuantaIndex::from_transports(query.clone(), unused_control(), unused_ingest());
    let request = quanta_index_contract::TextQueryRequest {
        syntax: quanta_index_contract::TextQuerySyntax::Sourcegraph,
        query_text: "repo:repo-1 lang:rust sample".to_string(),
        generation: None,
        generation_selector: Some(GenerationSelector::Active {
            repo_id: repo_id(),
            revision_id: revision_id(),
        }),
        top_k: 13,
    };
    let _response = ok_or_fail!(client.lexical().query_request(request.clone()));
    let captured = ok_or_fail!(only_query_request(query.as_ref()));
    assert_eq!(
        captured.payload,
        quanta_index_contract::SearchPlaneQueryIpcRequest::Text(request)
    );
}

#[test]
fn symbol_query_request_forwards_contract_dto_unchanged() {
    let query = Arc::new(StubQueryTransport::new(
        SearchPlaneQueryIpcResponse::Symbol(quanta_index_contract::SymbolQueryResponse {
            generation: sample_generation_pin(),
            results: vec![sample_symbol_hit()],
        }),
    ));
    let client = QuantaIndex::from_transports(query.clone(), unused_control(), unused_ingest());
    let request = quanta_index_contract::SymbolQueryRequest {
        syntax: quanta_index_contract::TextQuerySyntax::Native,
        query_text: "symbol:sample".to_string(),
        generation: Some(sample_generation_pin()),
        generation_selector: None,
        top_k: 9,
    };
    let response = ok_or_fail!(client.symbol().query_request(request.clone()));
    assert_eq!(response.results.len(), 1);
    assert!(!response.results.is_empty(), "expected one symbol result");
    let Some(first) = response.results.first() else {
        return;
    };
    assert_eq!(first.candidate_id, "sym-1");
    assert_eq!(first.symbol_kind.as_str(), "function");
    assert_eq!(first.symbol_kind_family, Some(SymbolKindFamily::Callable));
    let captured = ok_or_fail!(only_query_request(query.as_ref()));
    assert_eq!(
        captured.payload,
        quanta_index_contract::SearchPlaneQueryIpcRequest::Symbol(request)
    );
}

#[test]
fn hybrid_seed_search_builder_dispatches_hybrid_seed_request_with_semantic_text() {
    let query = Arc::new(StubQueryTransport::new(
        SearchPlaneQueryIpcResponse::HybridSeed(HybridSeedQueryResponse {
            generation: sample_generation_pin(),
            manifest_digest: "manifest-digest".to_string(),
            seed_candidates: vec![sample_hybrid_seed_candidate()],
            seed_candidates_v2: None,
            explanation: sample_explanation(),
        }),
    ));
    let client = QuantaIndex::from_transports(query.clone(), unused_control(), unused_ingest());
    let _response = ok_or_fail!(
        client
            .search()
            .hybrid_seed()
            .native("scope text")
            .semantic_text("0.25 0.75")
            .active(repo_id(), revision_id())
            .dense_corpus(quanta_index_contract::SemanticCorpusKindV1::SymbolCard, 40,)
            .dense_corpus(quanta_index_contract::SemanticCorpusKindV1::ModuleCard, 20,)
            .top_k(7)
            .execute()
    );
    let captured = ok_or_fail!(only_query_request(query.as_ref()));
    assert!(
        matches!(
            &captured.payload,
            quanta_index_contract::SearchPlaneQueryIpcRequest::HybridSeed(_)
        ),
        "expected hybrid seed request, got {:?}",
        captured.payload
    );
    let quanta_index_contract::SearchPlaneQueryIpcRequest::HybridSeed(req) = &captured.payload
    else {
        return;
    };
    assert_eq!(req.top_k, 7);
    assert_eq!(req.text_query.top_k, 7);
    assert_eq!(req.semantic_query_text.as_str(), "0.25 0.75");
    assert_eq!(
        req.dense_corpora,
        vec![
            quanta_index_contract::SemanticSeedCorpusBudgetV1 {
                corpus_kind: quanta_index_contract::SemanticCorpusKindV1::SymbolCard,
                top_k: 40,
            },
            quanta_index_contract::SemanticSeedCorpusBudgetV1 {
                corpus_kind: quanta_index_contract::SemanticCorpusKindV1::ModuleCard,
                top_k: 20,
            },
        ]
    );
}

#[test]
fn semantic_query_request_forwards_contract_dto_unchanged() {
    let query = Arc::new(StubQueryTransport::new(
        SearchPlaneQueryIpcResponse::Semantic(SemanticQueryResponse {
            generation: sample_generation_pin(),
            results: vec![sample_hit()],
            explanation: sample_explanation(),
        }),
    ));
    let client = QuantaIndex::from_transports(query.clone(), unused_control(), unused_ingest());
    let request = quanta_index_contract::SemanticQueryRequest {
        query_text: "legacy semantic text".to_string(),
        generation: None,
        generation_selector: Some(GenerationSelector::Active {
            repo_id: repo_id(),
            revision_id: revision_id(),
        }),
        lexical_scope: Some(quanta_index_contract::TextQueryRequest {
            syntax: quanta_index_contract::TextQuerySyntax::Sourcegraph,
            query_text: "repo:repo-1 file:src/lib.rs".to_string(),
            generation: Some(sample_generation_pin()),
            generation_selector: None,
            top_k: 4,
        }),
        top_k: 6,
    };
    let _response = ok_or_fail!(client.semantic().query_request(request.clone()));
    let captured = ok_or_fail!(only_query_request(query.as_ref()));
    assert_eq!(
        captured.payload,
        quanta_index_contract::SearchPlaneQueryIpcRequest::Semantic(request)
    );
}

#[test]
fn hybrid_seed_request_forwards_contract_dto_unchanged() {
    let query = Arc::new(StubQueryTransport::new(
        SearchPlaneQueryIpcResponse::HybridSeed(HybridSeedQueryResponse {
            generation: sample_generation_pin(),
            manifest_digest: "manifest-digest".to_string(),
            seed_candidates: vec![sample_hybrid_seed_candidate()],
            seed_candidates_v2: None,
            explanation: sample_explanation(),
        }),
    ));
    let client = QuantaIndex::from_transports(query.clone(), unused_control(), unused_ingest());
    let request = quanta_index_contract::HybridSeedQueryRequest {
        text_query: quanta_index_contract::TextQueryRequest {
            syntax: quanta_index_contract::TextQuerySyntax::Native,
            query_text: "hybrid text".to_string(),
            generation: Some(sample_generation_pin()),
            generation_selector: None,
            top_k: 11,
        },
        semantic_query_text: "legacy hybrid semantic".to_string(),
        generation: None,
        generation_selector: Some(GenerationSelector::Active {
            repo_id: repo_id(),
            revision_id: revision_id(),
        }),
        dense_corpora: Vec::new(),
        top_k: 12,
    };
    let _response = ok_or_fail!(client.search().hybrid_seed_request(request.clone()));
    let captured = ok_or_fail!(only_query_request(query.as_ref()));
    assert_eq!(
        captured.payload,
        quanta_index_contract::SearchPlaneQueryIpcRequest::HybridSeed(request)
    );
}

#[test]
fn lexical_sourcegraph_query_builder_dispatches_text_query_request() {
    let query = Arc::new(StubQueryTransport::new(SearchPlaneQueryIpcResponse::Text(
        TextQueryResponse {
            generation: sample_generation_pin(),
            results: vec![sample_hit()],
            file_owner_rows: None,
        },
    )));
    let client = QuantaIndex::from_transports(query.clone(), unused_control(), unused_ingest());
    let response = client
        .lexical()
        .query()
        .sourcegraph("repo:repo-1 lang:rust sample")
        .pinned(sample_generation_pin())
        .top_k(9)
        .execute();
    let response = ok_or_fail!(response);
    assert_eq!(response.results.len(), 1);
    let captured = ok_or_fail!(only_query_request(query.as_ref()));
    assert!(
        matches!(
            &captured.payload,
            quanta_index_contract::SearchPlaneQueryIpcRequest::Text(_)
        ),
        "expected text request, got {:?}",
        captured.payload
    );
    let quanta_index_contract::SearchPlaneQueryIpcRequest::Text(req) = &captured.payload else {
        return;
    };
    assert_eq!(
        req.syntax,
        quanta_index_contract::TextQuerySyntax::Sourcegraph
    );
    assert_eq!(req.query_text.as_str(), "repo:repo-1 lang:rust sample");
    assert_eq!(req.generation, Some(sample_generation_pin()));
    assert_eq!(req.top_k, 9);
}

#[test]
fn search_corpus_publish_routes_through_ingest_transport_and_carries_typed_records() {
    let receipt = BatchPublishReceipt {
        generation: ManifestGeneration::new(1),
        manifest_digest: "sha256:feed".to_string(),
        accepted_replace_scopes: 2,
        accepted_tombstone_scopes: 0,
        sealed: true,
    };
    let ingest = Arc::new(StubIngestTransport::new(
        SearchPlaneIngestIpcResponse::SearchCorpusReceipt(receipt.clone()),
    ));
    let client = QuantaIndex::from_transports(unused_query(), unused_control(), ingest.clone());
    let chunk = sample_chunk();
    let symbol = sample_symbol();
    let batch = SearchCorpusBatch::replace_generation(
        repo_id(),
        revision_id(),
        ManifestGeneration::new(1),
        "manifest:feed",
        "batch:feed",
    )
    .replace_scope(
        sample_search_scope(),
        "scope:feed",
        vec![chunk.clone()],
        vec![symbol.clone()],
    );
    let observed = ok_or_fail!(client.search_corpus().publish(&batch));
    assert_eq!(observed, receipt);
    let captured = ok_or_fail!(only_ingest_request(ingest.as_ref()));
    assert!(
        matches!(
            &captured.payload,
            SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(_)
        ),
        "expected PublishSearchCorpusBatch, got {:?}",
        captured.payload
    );
    let SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(wire) = &captured.payload else {
        return;
    };
    assert_eq!(wire.repo_id, repo_id());
    assert_eq!(wire.manifest_digest, "manifest:feed");
    assert_eq!(wire.batch_digest, "batch:feed");
    assert_eq!(wire.replace_scopes.len(), 1);
    assert_eq!(wire.tombstone_scopes.len(), 0);
    assert!(wire.seal);
    assert_eq!(
        wire.replace_scopes.len(),
        1,
        "expected one search corpus replace scope"
    );
    let Some(first_scope) = wire.replace_scopes.first() else {
        return;
    };
    assert_eq!(first_scope.scope, sample_search_scope());
    assert_eq!(first_scope.chunks, vec![chunk]);
    assert_eq!(first_scope.symbols, vec![symbol]);
}

#[test]
fn reader_client_routes_lexical_query_surface() {
    let query = Arc::new(StubQueryTransport::new(SearchPlaneQueryIpcResponse::Text(
        TextQueryResponse {
            generation: sample_generation_pin(),
            results: vec![sample_hit()],
            file_owner_rows: None,
        },
    )));
    let client = QuantaIndex::from_transports(query.clone(), unused_control(), unused_ingest());
    let response = ok_or_fail!(
        client
            .reader()
            .lexical()
            .native("reader needle")
            .active(repo_id(), revision_id())
            .top_k(4)
            .execute()
    );
    assert_eq!(response.generation, sample_generation_pin());
    let captured = ok_or_fail!(only_query_request(query.as_ref()));
    assert!(
        matches!(
            captured.payload,
            quanta_index_contract::SearchPlaneQueryIpcRequest::Text(_)
        ),
        "expected text request, got {:?}",
        captured.payload
    );
    let quanta_index_contract::SearchPlaneQueryIpcRequest::Text(req) = &captured.payload else {
        return;
    };
    assert_eq!(req.query_text, "reader needle");
    assert_eq!(req.top_k, 4);
}

#[test]
fn producer_client_publish_search_corpus_accepts_unsealed_batches() {
    let ingest = Arc::new(StubIngestTransport::new(
        SearchPlaneIngestIpcResponse::SearchCorpusReceipt(BatchPublishReceipt::default()),
    ));
    let client = QuantaIndex::from_transports(unused_query(), unused_control(), ingest.clone());
    let batch = SearchCorpusBatch::replace_generation(
        repo_id(),
        revision_id(),
        ManifestGeneration::new(2),
        "manifest:unsealed",
        "batch:unsealed",
    )
    .without_seal();
    let _receipt = ok_or_fail!(client.producer().publish_search_corpus(&batch));
    let captured = ok_or_fail!(only_ingest_request(ingest.as_ref()));
    assert!(
        matches!(
            captured.payload,
            SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(_)
        ),
        "expected search corpus ingest request, got {:?}",
        captured.payload
    );
    let SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(wire) = &captured.payload else {
        return;
    };
    assert!(
        !wire.seal,
        "unsealed producer publish must preserve seal=false"
    );
}

#[test]
fn producer_client_publish_search_corpus_and_activate_routes_ingest_then_control() {
    let receipt = BatchPublishReceipt {
        generation: ManifestGeneration::new(7),
        manifest_digest: "manifest:activate".to_string(),
        accepted_replace_scopes: 0,
        accepted_tombstone_scopes: 0,
        sealed: true,
    };
    let ack = SearchPlaneActivationAck {
        repo_id: repo_id(),
        revision_id: revision_id(),
        manifest_generation: receipt.generation,
        manifest_digest: receipt.manifest_digest.clone(),
        tracks: vec![Track::Lexical],
    };
    let control = Arc::new(StubControlTransport::new(
        quanta_index_contract::SearchPlaneControlIpcResponse::ActivationAck(ack.clone()),
    ));
    let ingest = Arc::new(StubIngestTransport::new(
        SearchPlaneIngestIpcResponse::SearchCorpusReceipt(receipt.clone()),
    ));
    let client = QuantaIndex::from_transports(unused_query(), control.clone(), ingest.clone());
    let batch = SearchCorpusBatch::replace_generation(
        repo_id(),
        revision_id(),
        ManifestGeneration::new(7),
        "manifest:activate",
        "batch:activate",
    );
    let (observed_receipt, observed_ack) =
        ok_or_fail!(client.producer().publish_search_corpus_and_activate(&batch));
    assert_eq!(observed_receipt, receipt);
    assert_eq!(observed_ack, ack);

    let ingest_request = ok_or_fail!(only_ingest_request(ingest.as_ref()));
    assert!(matches!(
        ingest_request.payload,
        SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(_)
    ));
    let control_request = ok_or_fail!(only_control_request(control.as_ref()));
    let quanta_index_contract::SearchPlaneControlIpcRequest::ActivateGeneration(request) =
        &control_request.payload
    else {
        return;
    };
    assert_eq!(request.repo_id, repo_id());
    assert_eq!(request.revision_id, revision_id());
    assert_eq!(request.manifest_generation, ManifestGeneration::new(7));
    assert_eq!(request.manifest_digest, "manifest:activate");
    assert_eq!(request.tracks, vec![Track::Lexical]);
}

#[test]
fn control_client_activate_rejects_empty_track_iterables() {
    let client = QuantaIndex::from_transports(unused_query(), unused_control(), unused_ingest());
    let err = client
        .control()
        .activate()
        .repo(repo_id())
        .revision(revision_id())
        .generation(ManifestGeneration::new(9))
        .manifest_digest("manifest:empty")
        .tracks([])
        .err();
    assert!(
        matches!(err, Some(crate::SdkError::Usage(ref message)) if message.contains("at least one track")),
        "expected Usage error for empty tracks, got {err:?}"
    );
}

#[test]
fn history_publish_routes_through_ingest_transport_and_carries_typed_authority_records() {
    let receipt = BatchPublishReceipt {
        generation: ManifestGeneration::new(3),
        manifest_digest: String::new(),
        accepted_replace_scopes: 4,
        accepted_tombstone_scopes: 0,
        sealed: false,
    };
    let ingest = Arc::new(StubIngestTransport::new(
        SearchPlaneIngestIpcResponse::HistoryReceipt(receipt.clone()),
    ));
    let client = QuantaIndex::from_transports(unused_query(), unused_control(), ingest.clone());
    let batch = HistoryBatch::new(
        repo_id(),
        revision_id(),
        ManifestGeneration::new(3),
        "batch:history-3",
    )
    .manifest_digest("manifest:history-3")
    .commit(sample_commit_record())
    .ref_upsert("refs/heads/main", sample_commit_sha())
    .tag_upsert("v1.0.0", sample_commit_sha())
    .diff_hunk(sample_commit_sha(), "src/lib.rs", sample_diff_record());
    let observed = ok_or_fail!(client.history().publish(&batch));
    assert_eq!(observed, receipt);
    let captured = ok_or_fail!(only_ingest_request(ingest.as_ref()));
    assert!(
        matches!(
            &captured.payload,
            SearchPlaneIngestIpcRequest::PublishHistoryBatch(_)
        ),
        "expected PublishHistoryBatch, got {:?}",
        captured.payload
    );
    let SearchPlaneIngestIpcRequest::PublishHistoryBatch(wire) = &captured.payload else {
        return;
    };
    assert_eq!(wire.manifest_digest.as_deref(), Some("manifest:history-3"));
    assert_eq!(wire.batch_digest, "batch:history-3");
    assert_eq!(wire.commits.len(), 1);
    assert_eq!(wire.refs.len(), 1);
    assert_eq!(wire.tags.len(), 1);
    assert_eq!(wire.diff_hunks.len(), 1);
    assert_eq!(wire.commits.len(), 1, "expected one history commit");
    let Some(first_commit) = wire.commits.first() else {
        return;
    };
    assert_eq!(first_commit.author_time_ms, 11);
    assert_eq!(wire.diff_hunks.len(), 1, "expected one history diff hunk");
    let Some(first_diff) = wire.diff_hunks.first() else {
        return;
    };
    assert_eq!(first_diff.record.hunk_header.as_ref(), "@@ -1,1 +1,2 @@");
}

#[test]
fn history_publish_repo_commit_recency_routes_through_ingest_transport() {
    let receipt = BatchPublishReceipt {
        generation: ManifestGeneration::new(3),
        manifest_digest: "batch:repo-commit-recency-3".to_string(),
        accepted_replace_scopes: 2,
        accepted_tombstone_scopes: 0,
        sealed: false,
    };
    let ingest = Arc::new(StubIngestTransport::new(
        SearchPlaneIngestIpcResponse::RepoCommitRecencyReceipt(receipt.clone()),
    ));
    let client = QuantaIndex::from_transports(unused_query(), unused_control(), ingest.clone());
    let batch = crate::RepoCommitRecencyBatch::new(
        repo_id(),
        revision_id(),
        ManifestGeneration::new(3),
        "batch:repo-commit-recency-3",
    )
    .entry(RepoId::new("corp-a"), 1_717_171_717_000)
    .entry(RepoId::new("corp-b"), 1_617_171_717_000);
    let observed = ok_or_fail!(client.history().publish_repo_commit_recency(&batch));
    assert_eq!(observed, receipt);
    let captured = ok_or_fail!(only_ingest_request(ingest.as_ref()));
    assert!(
        matches!(
            &captured.payload,
            SearchPlaneIngestIpcRequest::PublishRepoCommitRecencyBatch(_)
        ),
        "expected PublishRepoCommitRecencyBatch, got {:?}",
        captured.payload
    );
    let SearchPlaneIngestIpcRequest::PublishRepoCommitRecencyBatch(wire) = &captured.payload else {
        return;
    };
    assert_eq!(wire.batch_digest, "batch:repo-commit-recency-3");
    let [entry_a, entry_b] = wire.entries.as_slice() else {
        assert!(
            false,
            "expected two repo-commit-recency entries, got {}",
            wire.entries.len()
        );
        return;
    };
    assert_eq!(entry_a.source_repo_id.as_str(), "corp-a");
    assert_eq!(entry_a.latest_committer_time_ms, 1_717_171_717_000);
    assert_eq!(entry_b.source_repo_id.as_str(), "corp-b");
    assert_eq!(entry_b.latest_committer_time_ms, 1_617_171_717_000);
}

#[test]
fn history_publish_repo_meta_routes_through_ingest_transport() {
    let receipt = BatchPublishReceipt {
        generation: ManifestGeneration::new(4),
        manifest_digest: "batch:repo-meta-4".to_string(),
        accepted_replace_scopes: 2,
        accepted_tombstone_scopes: 0,
        sealed: false,
    };
    let ingest = Arc::new(StubIngestTransport::new(
        SearchPlaneIngestIpcResponse::RepoMetaReceipt(receipt.clone()),
    ));
    let client = QuantaIndex::from_transports(unused_query(), unused_control(), ingest.clone());
    let batch = crate::RepoMetaBatch::new(
        repo_id(),
        revision_id(),
        ManifestGeneration::new(4),
        "batch:repo-meta-4",
    )
    .entry(RepoId::new("corp-a"), "license", "apache-2.0")
    .entry(RepoId::new("corp-b"), "license", "gpl-3.0");
    let observed = ok_or_fail!(client.history().publish_repo_meta(&batch));
    assert_eq!(observed, receipt);
    let captured = ok_or_fail!(only_ingest_request(ingest.as_ref()));
    assert!(
        matches!(
            &captured.payload,
            SearchPlaneIngestIpcRequest::PublishRepoMetaBatch(_)
        ),
        "expected PublishRepoMetaBatch, got {:?}",
        captured.payload
    );
    let SearchPlaneIngestIpcRequest::PublishRepoMetaBatch(wire) = &captured.payload else {
        return;
    };
    assert_eq!(wire.batch_digest, "batch:repo-meta-4");
    let [entry_a, entry_b] = wire.entries.as_slice() else {
        assert!(
            false,
            "expected two repo-meta entries, got {}",
            wire.entries.len()
        );
        return;
    };
    assert_eq!(entry_a.source_repo_id.as_str(), "corp-a");
    assert_eq!(entry_a.key, "license");
    assert_eq!(entry_a.value, "apache-2.0");
    assert_eq!(entry_b.source_repo_id.as_str(), "corp-b");
    assert_eq!(entry_b.key, "license");
    assert_eq!(entry_b.value, "gpl-3.0");
}

#[test]
fn history_publish_repo_topic_routes_through_ingest_transport() {
    let receipt = BatchPublishReceipt {
        generation: ManifestGeneration::new(5),
        manifest_digest: "batch:repo-topic-5".to_string(),
        accepted_replace_scopes: 3,
        accepted_tombstone_scopes: 0,
        sealed: false,
    };
    let ingest = Arc::new(StubIngestTransport::new(
        SearchPlaneIngestIpcResponse::RepoTopicReceipt(receipt.clone()),
    ));
    let client = QuantaIndex::from_transports(unused_query(), unused_control(), ingest.clone());
    let batch = crate::RepoTopicBatch::new(
        repo_id(),
        revision_id(),
        ManifestGeneration::new(5),
        "batch:repo-topic-5",
    )
    .entry(RepoId::new("corp-a"), "security")
    .entry(RepoId::new("corp-a"), "platform")
    .entry(RepoId::new("corp-b"), "ml");
    let observed = ok_or_fail!(client.history().publish_repo_topic(&batch));
    assert_eq!(observed, receipt);
    let captured = ok_or_fail!(only_ingest_request(ingest.as_ref()));
    assert!(
        matches!(
            &captured.payload,
            SearchPlaneIngestIpcRequest::PublishRepoTopicBatch(_)
        ),
        "expected PublishRepoTopicBatch, got {:?}",
        captured.payload
    );
    let SearchPlaneIngestIpcRequest::PublishRepoTopicBatch(wire) = &captured.payload else {
        return;
    };
    assert_eq!(wire.batch_digest, "batch:repo-topic-5");
    let [entry_a, entry_b, entry_c] = wire.entries.as_slice() else {
        assert!(
            false,
            "expected three repo-topic entries, got {}",
            wire.entries.len()
        );
        return;
    };
    assert_eq!(entry_a.source_repo_id.as_str(), "corp-a");
    assert_eq!(entry_a.topic, "security");
    assert_eq!(entry_b.source_repo_id.as_str(), "corp-a");
    assert_eq!(entry_b.topic, "platform");
    assert_eq!(entry_c.source_repo_id.as_str(), "corp-b");
    assert_eq!(entry_c.topic, "ml");
}

#[test]
fn history_publish_file_ownership_routes_through_ingest_transport() {
    let receipt = BatchPublishReceipt {
        generation: ManifestGeneration::new(5),
        manifest_digest: "batch:file-ownership-5".to_string(),
        accepted_replace_scopes: 2,
        accepted_tombstone_scopes: 0,
        sealed: false,
    };
    let ingest = Arc::new(StubIngestTransport::new(
        SearchPlaneIngestIpcResponse::FileOwnershipReceipt(receipt.clone()),
    ));
    let client = QuantaIndex::from_transports(unused_query(), unused_control(), ingest.clone());
    let batch = crate::FileOwnershipBatch::new(
        repo_id(),
        revision_id(),
        ManifestGeneration::new(5),
        "batch:file-ownership-5",
    )
    .entry(
        RepoId::new("corp-a"),
        RepoRelativePath::new("src/gate-a.rs"),
        vec!["@alice".to_string(), "@acme/platform".to_string()],
    )
    .entry(
        RepoId::new("corp-b"),
        RepoRelativePath::new("src/gate-b.rs"),
        Vec::new(),
    );
    let observed = ok_or_fail!(client.history().publish_file_ownership(&batch));
    assert_eq!(observed, receipt);
    let captured = ok_or_fail!(only_ingest_request(ingest.as_ref()));
    assert!(
        matches!(
            &captured.payload,
            SearchPlaneIngestIpcRequest::PublishFileOwnershipBatch(_)
        ),
        "expected PublishFileOwnershipBatch, got {:?}",
        captured.payload
    );
    let SearchPlaneIngestIpcRequest::PublishFileOwnershipBatch(wire) = &captured.payload else {
        return;
    };
    assert_eq!(wire.batch_digest, "batch:file-ownership-5");
    let [entry_a, entry_b] = wire.entries.as_slice() else {
        assert!(
            false,
            "expected two file-ownership entries, got {}",
            wire.entries.len()
        );
        return;
    };
    assert_eq!(entry_a.source_repo_id.as_str(), "corp-a");
    assert_eq!(entry_a.repo_relative_path.as_str(), "src/gate-a.rs");
    assert_eq!(entry_a.owners, vec!["@alice", "@acme/platform"]);
    assert_eq!(entry_b.source_repo_id.as_str(), "corp-b");
    assert_eq!(entry_b.repo_relative_path.as_str(), "src/gate-b.rs");
    assert!(entry_b.owners.is_empty());
}

#[test]
fn history_publish_file_contributor_routes_through_ingest_transport() {
    let receipt = BatchPublishReceipt {
        generation: ManifestGeneration::new(6),
        manifest_digest: "batch:file-contributor-6".to_string(),
        accepted_replace_scopes: 2,
        accepted_tombstone_scopes: 0,
        sealed: false,
    };
    let ingest = Arc::new(StubIngestTransport::new(
        SearchPlaneIngestIpcResponse::FileContributorReceipt(receipt.clone()),
    ));
    let client = QuantaIndex::from_transports(unused_query(), unused_control(), ingest.clone());
    let batch = crate::FileContributorBatch::new(
        repo_id(),
        revision_id(),
        ManifestGeneration::new(6),
        "batch:file-contributor-6",
    )
    .entry(
        RepoId::new("corp-a"),
        RepoRelativePath::new("src/gate-a.rs"),
        vec!["alice".to_string(), "carol".to_string()],
    )
    .entry(
        RepoId::new("corp-b"),
        RepoRelativePath::new("src/gate-b.rs"),
        vec!["bob".to_string()],
    );
    let observed = ok_or_fail!(client.history().publish_file_contributor(&batch));
    assert_eq!(observed, receipt);
    let captured = ok_or_fail!(only_ingest_request(ingest.as_ref()));
    assert!(
        matches!(
            &captured.payload,
            SearchPlaneIngestIpcRequest::PublishFileContributorBatch(_)
        ),
        "expected PublishFileContributorBatch, got {:?}",
        captured.payload
    );
    let SearchPlaneIngestIpcRequest::PublishFileContributorBatch(wire) = &captured.payload else {
        return;
    };
    assert_eq!(wire.batch_digest, "batch:file-contributor-6");
    let [entry_a, entry_b] = wire.entries.as_slice() else {
        assert!(
            false,
            "expected two file-contributor entries, got {}",
            wire.entries.len()
        );
        return;
    };
    assert_eq!(entry_a.source_repo_id.as_str(), "corp-a");
    assert_eq!(entry_a.repo_relative_path.as_str(), "src/gate-a.rs");
    let canon_a: Vec<&str> = entry_a
        .contributors
        .iter()
        .map(|c| c.canonical.as_str())
        .collect();
    assert_eq!(canon_a, vec!["alice", "carol"]);
    assert_eq!(entry_b.source_repo_id.as_str(), "corp-b");
    assert_eq!(entry_b.repo_relative_path.as_str(), "src/gate-b.rs");
    let canon_b: Vec<&str> = entry_b
        .contributors
        .iter()
        .map(|c| c.canonical.as_str())
        .collect();
    assert_eq!(canon_b, vec!["bob"]);
}

#[test]
fn dirty_publish_routes_through_ingest_transport_and_carries_typed_entries() {
    let ingest = Arc::new(StubIngestTransport::new(
        SearchPlaneIngestIpcResponse::DirtyReceipt(BatchPublishReceipt::default()),
    ));
    let client = QuantaIndex::from_transports(unused_query(), unused_control(), ingest.clone());
    let batch = DirtyBatch::new(
        repo_id(),
        revision_id(),
        ManifestGeneration::new(4),
        1_717_171_717_000,
        "batch:dirty-4",
    )
    .upsert(sample_dirty_record())
    .delete(ChunkId::new("chunk-evict"));
    let _receipt = ok_or_fail!(client.runtime().publish_dirty(&batch));
    let captured = ok_or_fail!(only_ingest_request(ingest.as_ref()));
    assert!(
        matches!(
            &captured.payload,
            SearchPlaneIngestIpcRequest::PublishDirtyBatch(_)
        ),
        "expected PublishDirtyBatch, got {:?}",
        captured.payload
    );
    let SearchPlaneIngestIpcRequest::PublishDirtyBatch(wire) = &captured.payload else {
        return;
    };
    assert_eq!(wire.overlay_epoch_ms, 1_717_171_717_000);
    assert_eq!(wire.batch_digest, "batch:dirty-4");
    assert_eq!(wire.entries.len(), 2);
}

#[test]
fn structural_publish_routes_through_ingest_transport_and_carries_parse_trees() {
    let ingest = Arc::new(StubIngestTransport::new(
        SearchPlaneIngestIpcResponse::StructuralReceipt(BatchPublishReceipt::default()),
    ));
    let client = QuantaIndex::from_transports(unused_query(), unused_control(), ingest.clone());
    let batch = StructuralBatch::delta(
        repo_id(),
        revision_id(),
        ManifestGeneration::new(5),
        ManifestGeneration::new(4),
        "manifest:structural",
        "batch:structural",
    )
    .replace_scope(
        sample_search_scope(),
        "scope:structural",
        vec![quanta_index_contract::StructuralTreeRecord {
            chunk_id: ChunkId::new("chunk-tree"),
            record: sample_parse_tree_record(),
        }],
    )
    .tombstone_scope(SearchScopeKey {
        doc_surface: SearchScopeSurface::Chunk,
        repo_relative_path: RepoRelativePath::new("src/old.rs"),
    });
    let _receipt = ok_or_fail!(client.structural().publish(&batch));
    let captured = ok_or_fail!(only_ingest_request(ingest.as_ref()));
    assert!(
        matches!(
            &captured.payload,
            SearchPlaneIngestIpcRequest::PublishStructuralBatch(_)
        ),
        "expected PublishStructuralBatch, got {:?}",
        captured.payload
    );
    let SearchPlaneIngestIpcRequest::PublishStructuralBatch(wire) = &captured.payload else {
        return;
    };
    assert_eq!(wire.replace_scopes.len(), 1);
    assert_eq!(wire.tombstone_scopes.len(), 1);
    let Some(first_scope) = wire.replace_scopes.first() else {
        return;
    };
    assert_eq!(first_scope.trees.len(), 1);
}

#[test]
fn repomap_publish_routes_through_ingest_transport() {
    let ack = RepoMapMutationAck {
        repo_id: repo_id(),
        revision_id: revision_id(),
        manifest_generation: ManifestGeneration::new(1),
    };
    let ingest = Arc::new(StubIngestTransport::new(
        SearchPlaneIngestIpcResponse::RepoMapReceipt(ack.clone()),
    ));
    let client = QuantaIndex::from_transports(unused_query(), unused_control(), ingest.clone());
    let bundle = quanta_index_contract::RepoMapSourceBundle::new(
        repo_id(),
        revision_id(),
        ManifestGeneration::new(1),
        "manifest-digest",
        "snap",
        1,
        "digest",
        quanta_index_contract::RepoMapGraphCoverage {
            item_index_availability: RepoMapItemIndexAvailability::Available,
            graph_coverage_class: RepoMapGraphCoverageClass::Full,
        },
        RepoMapExactnessSummary::Exact,
        RepoMapRedactionState::Unredacted,
    )
    .with_node(quanta_index_contract::RepoMapNode::File(
        quanta_index_contract::RepoMapFileNode {
            file_id: quanta_index_contract::FileId::new("file://src/lib.rs"),
            repo_relative_path: RepoRelativePath::new("src/lib.rs"),
            line_count: 12,
        },
    ))
    .with_node(quanta_index_contract::RepoMapNode::Symbol(
        quanta_index_contract::RepoMapSymbolNode {
            symbol_id: SymbolId::new("symbol://repomap"),
            owner_path: RepoRelativePath::new("src/lib.rs"),
            local_name: "RepoMapOwner".to_string(),
            qualified_name: "crate::RepoMapOwner".to_string(),
            symbol_kind: ok_or_fail!(SymbolKindCode::new("struct")),
        },
    ))
    .with_node(quanta_index_contract::RepoMapNode::Chunk(
        quanta_index_contract::RepoMapChunkNode {
            chunk_id: ChunkId::new("chunk://repomap"),
            owner_path: RepoRelativePath::new("src/lib.rs"),
            language: ok_or_fail!(LanguageCode::new("rust")),
            start_byte: 0,
            end_byte: 32,
            start_line: 1,
            end_line: 3,
            token_count: 16,
            preview_text: "repomap preview".to_string(),
            exactness: RepoMapChunkExactness::Exact,
        },
    ))
    .with_edge(quanta_index_contract::RepoMapEdge::Contains(
        quanta_index_contract::RepoMapContainsEdge {
            container: quanta_index_contract::RepoMapNodeRef::File(
                quanta_index_contract::FileId::new("file://src/lib.rs"),
            ),
            contained: quanta_index_contract::RepoMapNodeRef::Symbol(SymbolId::new(
                "symbol://repomap",
            )),
        },
    ))
    .with_edge(quanta_index_contract::RepoMapEdge::OwnsChunk(
        quanta_index_contract::RepoMapOwnsChunkEdge {
            owner: quanta_index_contract::RepoMapNodeRef::Symbol(SymbolId::new("symbol://repomap")),
            chunk: quanta_index_contract::RepoMapNodeRef::Chunk(ChunkId::new("chunk://repomap")),
        },
    ));
    let observed = ok_or_fail!(client.repomap().publish(&bundle));
    assert_eq!(observed.manifest_generation, ack.manifest_generation);
    let captured = ok_or_fail!(only_ingest_request(ingest.as_ref()));
    assert!(matches!(
        captured.payload,
        SearchPlaneIngestIpcRequest::PublishRepoMapBundle(_)
    ));
}

#[test]
fn repomap_query_routes_through_query_transport() {
    let response = sample_repomap_query_response();
    let query = Arc::new(StubQueryTransport::new(
        SearchPlaneQueryIpcResponse::RepoMapQuery(response.clone()),
    ));
    let client = QuantaIndex::from_transports(query.clone(), unused_control(), unused_ingest());
    let request = sample_repomap_query_request();
    let observed = ok_or_fail!(client.repomap().query(request.clone()));
    assert_eq!(observed, response);
    let captured = ok_or_fail!(only_query_request(query.as_ref()));
    assert!(
        matches!(
            &captured.payload,
            quanta_index_contract::SearchPlaneQueryIpcRequest::RepoMapQuery(_)
        ),
        "expected RepoMapQuery request, got {:?}",
        captured.payload
    );
    let quanta_index_contract::SearchPlaneQueryIpcRequest::RepoMapQuery(wire) = &captured.payload
    else {
        return;
    };
    assert_eq!(wire.query_text, request.query_text);
    assert_eq!(wire.top_k, request.top_k);
    assert_eq!(wire.token_budget, request.token_budget);
    assert_eq!(wire.focus_subjects, request.focus_subjects);
}

#[test]
fn repomap_activate_routes_through_control_transport() {
    let ack = RepoMapMutationAck {
        repo_id: repo_id(),
        revision_id: revision_id(),
        manifest_generation: ManifestGeneration::new(9),
    };
    let control = Arc::new(StubControlTransport::new(
        quanta_index_contract::SearchPlaneControlIpcResponse::RepoMapMutationAck(ack.clone()),
    ));
    let client = QuantaIndex::from_transports(unused_query(), control.clone(), unused_ingest());
    let request = quanta_index_contract::RepoMapActivateGenerationRequest {
        repo_id: repo_id(),
        revision_id: revision_id(),
        manifest_generation: ManifestGeneration::new(9),
        manifest_digest: "digest:repomap-9".to_string(),
    };
    let observed = ok_or_fail!(client.repomap().activate(request.clone()));
    assert_eq!(observed, ack);
    let captured = ok_or_fail!(only_control_request(control.as_ref()));
    assert!(
        matches!(
            &captured.payload,
            quanta_index_contract::SearchPlaneControlIpcRequest::RepoMapActivate(_)
        ),
        "expected RepoMapActivate request, got {:?}",
        captured.payload
    );
    let quanta_index_contract::SearchPlaneControlIpcRequest::RepoMapActivate(wire) =
        &captured.payload
    else {
        return;
    };
    assert_eq!(wire.repo_id, request.repo_id);
    assert_eq!(wire.revision_id, request.revision_id);
    assert_eq!(wire.manifest_generation, request.manifest_generation);
    assert_eq!(wire.manifest_digest, request.manifest_digest);
}

#[test]
fn history_query_routes_through_typed_query_variant() {
    let query = Arc::new(StubQueryTransport::new(
        SearchPlaneQueryIpcResponse::History(SearchPlaneHistoryQueryResponse {
            generation: sample_generation_pin(),
            commits: vec![],
            diffs: vec![],
        }),
    ));
    let client = QuantaIndex::from_transports(query.clone(), unused_control(), unused_ingest());
    let response = ok_or_fail!(
        client
            .history()
            .query()
            .native("type:commit author:alice")
            .pinned(sample_generation_pin())
            .top_k(5)
            .execute()
    );
    assert_eq!(response.generation, sample_generation_pin());
    let captured = ok_or_fail!(only_query_request(query.as_ref()));
    assert!(
        matches!(
            &captured.payload,
            quanta_index_contract::SearchPlaneQueryIpcRequest::History(_)
        ),
        "expected History request, got {:?}",
        captured.payload
    );
    let quanta_index_contract::SearchPlaneQueryIpcRequest::History(req) = &captured.payload else {
        return;
    };
    assert_eq!(req.text_query.query_text, "type:commit author:alice");
}

#[test]
fn history_sourcegraph_query_preserves_rev_filter_and_syntax() {
    let query = Arc::new(StubQueryTransport::new(
        SearchPlaneQueryIpcResponse::History(SearchPlaneHistoryQueryResponse {
            generation: sample_generation_pin(),
            commits: vec![],
            diffs: vec![],
        }),
    ));
    let client = QuantaIndex::from_transports(query.clone(), unused_control(), unused_ingest());
    let _response = ok_or_fail!(
        client
            .history()
            .query()
            .sourcegraph("type:commit rev:refs/heads/main")
            .pinned(sample_generation_pin())
            .top_k(3)
            .execute()
    );
    let captured = ok_or_fail!(only_query_request(query.as_ref()));
    assert!(
        matches!(
            &captured.payload,
            quanta_index_contract::SearchPlaneQueryIpcRequest::History(_)
        ),
        "expected History request, got {:?}",
        captured.payload
    );
    let quanta_index_contract::SearchPlaneQueryIpcRequest::History(req) = &captured.payload else {
        return;
    };
    assert_eq!(
        req.text_query.syntax,
        quanta_index_contract::TextQuerySyntax::Sourcegraph
    );
    assert_eq!(req.text_query.query_text, "type:commit rev:refs/heads/main");
}

#[test]
fn history_query_request_forwards_contract_dto_unchanged() {
    let query = Arc::new(StubQueryTransport::new(
        SearchPlaneQueryIpcResponse::History(SearchPlaneHistoryQueryResponse {
            generation: sample_generation_pin(),
            commits: vec![],
            diffs: vec![],
        }),
    ));
    let client = QuantaIndex::from_transports(query.clone(), unused_control(), unused_ingest());
    let request = HistoryQueryRequest {
        text_query: quanta_index_contract::TextQueryRequest {
            syntax: quanta_index_contract::TextQuerySyntax::Sourcegraph,
            query_text: "type:commit author:alice".to_string(),
            generation: None,
            generation_selector: Some(GenerationSelector::Active {
                repo_id: repo_id(),
                revision_id: revision_id(),
            }),
            top_k: 5,
        },
    };
    let _response = ok_or_fail!(client.history().query_request(request.clone()));
    let captured = ok_or_fail!(only_query_request(query.as_ref()));
    assert_eq!(
        captured.payload,
        quanta_index_contract::SearchPlaneQueryIpcRequest::History(request)
    );
}

#[test]
fn runtime_query_routes_through_typed_query_variant() {
    let query = Arc::new(StubQueryTransport::new(
        SearchPlaneQueryIpcResponse::RuntimeMetadata(SearchPlaneRuntimeMetadataQueryResponse {
            generation: sample_generation_pin(),
            results: vec![sample_hit()],
        }),
    ));
    let client = QuantaIndex::from_transports(query.clone(), unused_control(), unused_ingest());
    let response = ok_or_fail!(
        client
            .runtime()
            .query()
            .sourcegraph("dirty:yes")
            .pinned(sample_generation_pin())
            .top_k(3)
            .execute()
    );
    assert_eq!(response.results.len(), 1);
    let captured = ok_or_fail!(only_query_request(query.as_ref()));
    assert!(
        matches!(
            &captured.payload,
            quanta_index_contract::SearchPlaneQueryIpcRequest::RuntimeMetadata(_)
        ),
        "expected RuntimeMetadata request, got {:?}",
        captured.payload
    );
    let quanta_index_contract::SearchPlaneQueryIpcRequest::RuntimeMetadata(req) = &captured.payload
    else {
        return;
    };
    assert_eq!(
        req.text_query.syntax,
        quanta_index_contract::TextQuerySyntax::Sourcegraph
    );
}

#[test]
fn runtime_query_request_forwards_contract_dto_unchanged() {
    let query = Arc::new(StubQueryTransport::new(
        SearchPlaneQueryIpcResponse::RuntimeMetadata(SearchPlaneRuntimeMetadataQueryResponse {
            generation: sample_generation_pin(),
            results: vec![sample_hit()],
        }),
    ));
    let client = QuantaIndex::from_transports(query.clone(), unused_control(), unused_ingest());
    let request = RuntimeMetadataQueryRequest {
        text_query: quanta_index_contract::TextQueryRequest {
            syntax: quanta_index_contract::TextQuerySyntax::Native,
            query_text: "dirty:yes".to_string(),
            generation: Some(sample_generation_pin()),
            generation_selector: None,
            top_k: 3,
        },
    };
    let _response = ok_or_fail!(client.runtime().query_request(request.clone()));
    let captured = ok_or_fail!(only_query_request(query.as_ref()));
    assert_eq!(
        captured.payload,
        quanta_index_contract::SearchPlaneQueryIpcRequest::RuntimeMetadata(request)
    );
}

#[test]
fn structural_query_routes_through_typed_query_variant() {
    let query = Arc::new(StubQueryTransport::new(
        SearchPlaneQueryIpcResponse::Structural(SearchPlaneStructuralQueryResponse {
            generation: sample_generation_pin(),
            results: vec![],
        }),
    ));
    let client = QuantaIndex::from_transports(query.clone(), unused_control(), unused_ingest());
    let response = ok_or_fail!(
        client
            .structural()
            .query()
            .native("match { :[x] }")
            .pinned(sample_generation_pin())
            .top_k(2)
            .execute()
    );
    assert_eq!(response.generation, sample_generation_pin());
    let captured = ok_or_fail!(only_query_request(query.as_ref()));
    assert!(
        matches!(
            &captured.payload,
            quanta_index_contract::SearchPlaneQueryIpcRequest::Structural(_)
        ),
        "expected Structural request, got {:?}",
        captured.payload
    );
    let quanta_index_contract::SearchPlaneQueryIpcRequest::Structural(req) = &captured.payload
    else {
        return;
    };
    assert_eq!(req.text_query.query_text, "match { :[x] }");
}

#[test]
fn structural_query_request_forwards_contract_dto_unchanged() {
    let query = Arc::new(StubQueryTransport::new(
        SearchPlaneQueryIpcResponse::Structural(SearchPlaneStructuralQueryResponse {
            generation: sample_generation_pin(),
            results: vec![],
        }),
    ));
    let client = QuantaIndex::from_transports(query.clone(), unused_control(), unused_ingest());
    let request = StructuralQueryRequest {
        text_query: quanta_index_contract::TextQueryRequest {
            syntax: quanta_index_contract::TextQuerySyntax::Sourcegraph,
            query_text: r#"patterntype:structural "function_item""#.to_string(),
            generation: None,
            generation_selector: Some(GenerationSelector::Active {
                repo_id: repo_id(),
                revision_id: revision_id(),
            }),
            top_k: 4,
        },
    };
    let _response = ok_or_fail!(client.structural().query_request(request.clone()));
    let captured = ok_or_fail!(only_query_request(query.as_ref()));
    assert_eq!(
        captured.payload,
        quanta_index_contract::SearchPlaneQueryIpcRequest::Structural(request)
    );
}

#[test]
fn structural_native_query_preserves_syntax() {
    let query = Arc::new(StubQueryTransport::new(
        SearchPlaneQueryIpcResponse::Structural(SearchPlaneStructuralQueryResponse {
            generation: sample_generation_pin(),
            results: vec![],
        }),
    ));
    let client = QuantaIndex::from_transports(query.clone(), unused_control(), unused_ingest());
    let _response = ok_or_fail!(
        client
            .structural()
            .query()
            .native("repo:repo-1 lang:rust match { function_item }")
            .pinned(sample_generation_pin())
            .top_k(4)
            .execute()
    );
    let captured = ok_or_fail!(only_query_request(query.as_ref()));
    assert!(
        matches!(
            &captured.payload,
            quanta_index_contract::SearchPlaneQueryIpcRequest::Structural(_)
        ),
        "expected Structural request, got {:?}",
        captured.payload
    );
    let quanta_index_contract::SearchPlaneQueryIpcRequest::Structural(req) = &captured.payload
    else {
        return;
    };
    assert_eq!(
        req.text_query.syntax,
        quanta_index_contract::TextQuerySyntax::Native
    );
    assert_eq!(
        req.text_query.query_text,
        "repo:repo-1 lang:rust match { function_item }"
    );
    assert_eq!(req.text_query.top_k, 4);
}

#[test]
fn structural_sourcegraph_query_preserves_syntax() {
    let query = Arc::new(StubQueryTransport::new(
        SearchPlaneQueryIpcResponse::Structural(SearchPlaneStructuralQueryResponse {
            generation: sample_generation_pin(),
            results: vec![],
        }),
    ));
    let client = QuantaIndex::from_transports(query.clone(), unused_control(), unused_ingest());
    let _response =
        ok_or_fail!(client
        .structural()
        .query()
        .sourcegraph(
            r#"repo:repo-1 path:src/lib.rs lang:rust patterntype:structural "function_item""#
        )
        .pinned(sample_generation_pin())
        .top_k(4)
        .execute());
    let captured = ok_or_fail!(only_query_request(query.as_ref()));
    assert!(
        matches!(
            &captured.payload,
            quanta_index_contract::SearchPlaneQueryIpcRequest::Structural(_)
        ),
        "expected Structural request, got {:?}",
        captured.payload
    );
    let quanta_index_contract::SearchPlaneQueryIpcRequest::Structural(req) = &captured.payload
    else {
        return;
    };
    assert_eq!(
        req.text_query.syntax,
        quanta_index_contract::TextQuerySyntax::Sourcegraph
    );
    assert_eq!(
        req.text_query.query_text,
        r#"repo:repo-1 path:src/lib.rs lang:rust patterntype:structural "function_item""#
    );
    assert_eq!(req.text_query.top_k, 4);
}

#[test]
fn lexical_publish_propagates_ingest_error_as_typed_remote() {
    let ingest = Arc::new(StubIngestTransport::new(
        SearchPlaneIngestIpcResponse::Error(quanta_index_contract::SearchPlaneIpcError {
            code: "INVALID_REQUEST".to_string(),
            message: "channel rejected".to_string(),
            repair: None,
        }),
    ));
    let client = QuantaIndex::from_transports(unused_query(), unused_control(), ingest);
    let batch = SearchCorpusBatch::replace_generation(
        repo_id(),
        revision_id(),
        ManifestGeneration::new(1),
        "manifest:feed",
        "batch:feed",
    );
    let err = client.search_corpus().publish(&batch).err();
    assert!(
        matches!(err, Some(crate::SdkError::Remote { .. })),
        "expected Remote error, got {err:?}"
    );
    let Some(crate::SdkError::Remote { code, message, .. }) = err else {
        return;
    };
    assert_eq!(code, "INVALID_REQUEST");
    assert!(message.contains("channel rejected"));
}

#[test]
fn generations_current_returns_snapshot_from_control_response() {
    use crate::Track;
    use quanta_index_contract::{GenerationSnapshot, SearchPlaneControlIpcResponse};
    let snapshot = GenerationSnapshot {
        repo_id: repo_id(),
        revision_id: revision_id(),
        track: Track::Lexical,
        manifest_generation: ManifestGeneration::new(11),
        manifest_digest: "digest-11".to_string(),
    };
    let control = Arc::new(StubControlTransport::new(
        SearchPlaneControlIpcResponse::CurrentGenerationSnapshot(snapshot.clone()),
    ));
    let client = QuantaIndex::from_transports(unused_query(), control.clone(), unused_ingest());
    let observed = ok_or_fail!(client.generations().current(
        repo_id(),
        revision_id(),
        Track::Lexical,
    ));
    assert_eq!(observed, snapshot);
    let captured = ok_or_fail!(
        control
            .requests
            .lock()
            .map_err(|err| crate::SdkError::Protocol(format!(
                "control requests must not be poisoned: {err}"
            )))
    )
    .first()
    .cloned();
    assert!(matches!(
        captured.map(|request| request.payload),
        Some(quanta_index_contract::SearchPlaneControlIpcRequest::CurrentGeneration(_))
    ));
}

#[test]
fn generations_current_propagates_not_ready_as_typed_remote() {
    use crate::Track;
    use quanta_index_contract::{SearchPlaneControlIpcResponse, SearchPlaneIpcError};
    let control = Arc::new(StubControlTransport::new(
        SearchPlaneControlIpcResponse::Error(SearchPlaneIpcError {
            code: "NOT_READY".to_string(),
            message: "no active Lexical generation for repo=r revision=rev".to_string(),
            repair: None,
        }),
    ));
    let client = QuantaIndex::from_transports(unused_query(), control, unused_ingest());
    let err = client
        .generations()
        .current(repo_id(), revision_id(), Track::Lexical)
        .err();
    assert!(
        matches!(err, Some(crate::SdkError::Remote { .. })),
        "expected Remote error, got {err:?}"
    );
    let Some(crate::SdkError::Remote { code, .. }) = err else {
        return;
    };
    assert_eq!(code, "NOT_READY");
}

#[test]
fn generations_status_returns_report_with_track_records() {
    use crate::Track;
    use quanta_index_contract::{
        GenerationStatusReport, SearchPlaneControlIpcResponse, TrackReadinessRecord,
    };
    let report = GenerationStatusReport {
        repo_id: repo_id(),
        revision_id: revision_id(),
        tracks: vec![
            TrackReadinessRecord {
                track: Track::Lexical,
                manifest_generation: ManifestGeneration::new(7),
                manifest_digest: "digest-lex".to_string(),
            },
            TrackReadinessRecord {
                track: Track::Semantic,
                manifest_generation: ManifestGeneration::new(7),
                manifest_digest: "digest-sem".to_string(),
            },
        ],
    };
    let control = Arc::new(StubControlTransport::new(
        SearchPlaneControlIpcResponse::GenerationStatusReport(report.clone()),
    ));
    let client = QuantaIndex::from_transports(unused_query(), control, unused_ingest());
    let observed = ok_or_fail!(client.generations().status(repo_id(), revision_id()));
    assert_eq!(observed, report);
    assert_eq!(observed.tracks.len(), 2);
}

#[test]
fn generations_status_returns_empty_tracks_when_nothing_activated() {
    use quanta_index_contract::{GenerationStatusReport, SearchPlaneControlIpcResponse};
    let control = Arc::new(StubControlTransport::new(
        SearchPlaneControlIpcResponse::GenerationStatusReport(GenerationStatusReport {
            repo_id: repo_id(),
            revision_id: revision_id(),
            tracks: vec![],
        }),
    ));
    let client = QuantaIndex::from_transports(unused_query(), control, unused_ingest());
    let observed = ok_or_fail!(client.generations().status(repo_id(), revision_id()));
    assert!(
        observed.tracks.is_empty(),
        "QI-ACT-01: empty tracks is legitimate state, distinct from NOT_READY"
    );
}

#[test]
fn search_corpus_public_surface_keeps_legacy_lexical_ingest_names_out_v1() {
    let sdk_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let lexical_source =
        std::fs::read_to_string(sdk_root.join("src/lexical.rs")).expect("read lexical.rs");
    let client_source =
        std::fs::read_to_string(sdk_root.join("src/client.rs")).expect("read client.rs");
    let public_surface = std::fs::read_to_string(sdk_root.join("src/lib.rs")).expect("read lib.rs");
    let contract_ingest =
        std::fs::read_to_string(sdk_root.join("../quanta-index-contract/src/ipc/ingest.rs"))
            .expect("read contract ingest.rs");

    for forbidden in [
        "LexicalIngestBatch",
        "LexicalReplaceScope",
        "LexicalTombstoneScope",
        "PublishLexicalBatch",
        "publish_lexical",
        "DirectLexicalMaterializer",
        "LexicalIngestPort",
    ] {
        assert!(
            !lexical_source.contains(forbidden)
                && !client_source.contains(forbidden)
                && !public_surface.contains(forbidden)
                && !contract_ingest.contains(forbidden),
            "legacy lexical-ingest symbol must stay deleted from public ingest surfaces: {forbidden}",
        );
    }

    for required in [
        "SearchCorpusBatch",
        "publish_search_corpus",
        "publish_search_corpus_and_activate",
        "PublishSearchCorpusBatch",
        "SearchCorpusIngestBatch",
    ] {
        assert!(
            lexical_source.contains(required)
                || client_source.contains(required)
                || public_surface.contains(required)
                || contract_ingest.contains(required),
            "search-corpus ingest owner surface must keep `{required}` wired",
        );
    }
}
