use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use quanta_index_contract::lex::{
    CommitRecord, CommitSha, DiffHunkRecord, DirtyRecord, LanguageCode, ParseNode, ParseRoleTag,
    ParseTreeRecord, SymbolKindCode, SymbolKindFamily, SymbolRecord, SymbolRelationship,
    SymbolSpan, compute_parse_tree_source_hash,
};
use quanta_index_contract::{
    BatchPublishReceipt, CapabilityStatusV1, ChunkId, ChunkRecord, DiffHunkSide,
    ExactRepoRelativePathV1, GenerationSelector, GenerationSnapshot, HistoryQueryRequest,
    HybridSeedQueryResponse, ManifestGeneration, OwnerDocKind, PlannerStage, PlannerTraceEntry,
    QueryResultWindowV1, RepoId, RepoMapChunkExactness, RepoMapExactnessSummary,
    RepoMapGraphCoverageClass, RepoMapItemIndexAvailability, RepoMapMutationAck,
    RepoMapRedactionState, RepoRelativePath, RevisionId, RuntimeMetadataQueryRequest,
    SearchCorpusGenerationIdentityV1, SearchExplanation, SearchPlaneControlIpcRequestEnvelope,
    SearchPlaneControlIpcResponseEnvelope, SearchPlaneHistoryQueryResponse,
    SearchPlaneIngestIpcRequest, SearchPlaneIngestIpcRequestEnvelope, SearchPlaneIngestIpcResponse,
    SearchPlaneIngestIpcResponseEnvelope, SearchPlaneIpcError, SearchPlaneQueryIpcRequestEnvelope,
    SearchPlaneQueryIpcResponse, SearchPlaneQueryIpcResponseEnvelope,
    SearchPlaneRollbackSearchCorpusGenerationCasRequest, SearchPlaneRuntimeMetadataQueryResponse,
    SearchPlaneSearchCorpusActivationCasAck, SearchPlaneSearchCorpusRollbackCasAck,
    SearchPlaneStructuralQueryResponse, SearchScopeKey, SearchScopeSurface, SemanticCorpusKindV1,
    SemanticQueryResponse, SemanticSourceRecordV1, SemanticSourceScopeKeyV1, SourceRoleV1,
    StructuralQueryRequest, SymbolId, TextQueryResponse,
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
    request_id_offset: u64,
}

impl StubControlTransport {
    fn new(response: quanta_index_contract::SearchPlaneControlIpcResponse) -> Self {
        Self {
            requests: Mutex::new(Vec::new()),
            response: Mutex::new(Some(response)),
            request_id_offset: 0,
        }
    }

    fn with_request_id_offset(
        response: quanta_index_contract::SearchPlaneControlIpcResponse,
        request_id_offset: u64,
    ) -> Self {
        Self {
            requests: Mutex::new(Vec::new()),
            response: Mutex::new(Some(response)),
            request_id_offset,
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
            request_id: request.request_id.wrapping_add(self.request_id_offset),
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

fn sample_cluster_membership_request() -> quanta_index_contract::ClusterMembershipReadRequestV1 {
    quanta_index_contract::ClusterMembershipReadRequestV1 {
        cluster_record_id: "cluster-card:auth-service".to_string(),
        generation: sample_generation_pin(),
        expected_authority_digest: "cluster-authority-digest".to_string(),
        limit: 2,
    }
}

fn sample_cluster_membership_batch(
    count: usize,
) -> quanta_index_contract::ClusterMembershipBatchReadRequestV1 {
    quanta_index_contract::ClusterMembershipBatchReadRequestV1 {
        generation: sample_generation_pin(),
        items: (0..count)
            .map(
                |index| quanta_index_contract::ClusterMembershipBatchReadItemV1 {
                    cluster_record_id: format!("cluster-card:{index:02}"),
                    expected_authority_digest: format!("authority:{index:02}"),
                    limit: 1,
                },
            )
            .collect(),
    }
}

fn sample_cluster_membership_batch_response(
    request: &quanta_index_contract::ClusterMembershipBatchReadRequestV1,
) -> quanta_index_contract::ClusterMembershipBatchReadResponseV1 {
    quanta_index_contract::ClusterMembershipBatchReadResponseV1 {
        outcomes: request
            .items
            .iter()
            .map(|item| {
                quanta_index_contract::ClusterMembershipReadOutcomeV1::Available(
                    quanta_index_contract::ClusterMembershipSnapshotV1 {
                        cluster_record_id: item.cluster_record_id.clone(),
                        generation: request.generation.clone(),
                        authority_digest: item.expected_authority_digest.clone(),
                        members: vec![SymbolId::new(format!("symbol:{}", item.cluster_record_id))],
                        completeness:
                            quanta_index_contract::ClusterMembershipCompletenessV1::Complete,
                    },
                )
            })
            .collect(),
    }
}

fn search_corpus_identity(generation: u64, digest: &str) -> SearchCorpusGenerationIdentityV1 {
    SearchCorpusGenerationIdentityV1 {
        lexical: GenerationSnapshot {
            repo_id: repo_id(),
            revision_id: revision_id(),
            track: Track::Lexical,
            manifest_generation: ManifestGeneration::new(generation),
            manifest_digest: digest.to_string(),
        },
        semantic: GenerationSnapshot {
            repo_id: repo_id(),
            revision_id: revision_id(),
            track: Track::Semantic,
            manifest_generation: ManifestGeneration::new(generation),
            manifest_digest: digest.to_string(),
        },
    }
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

fn sample_hybrid_seed_candidate() -> quanta_index_contract::SeedCandidate {
    quanta_index_contract::SeedCandidate {
        record_id: "lex-1".to_string(),
        entity_id: "lex-1".to_string(),
        owner_kind: OwnerDocKind::Chunk,
        corpus_kind: None,
        authority_digest: None,
        repo_relative_path: RepoRelativePath::new("src/lib.rs"),
        snippet: "fn sample() {}".to_string(),
        seed_rank: 1,
        contributions: vec![
            quanta_index_contract::SeedContribution {
                lane: quanta_index_contract::SeedLane::Bm25,
                rank: 1,
                raw_score: Some(1.0),
                corpus_kind: None,
            },
            quanta_index_contract::SeedContribution {
                lane: quanta_index_contract::SeedLane::Dense,
                rank: 2,
                raw_score: Some(0.5),
                corpus_kind: None,
            },
        ],
        degraded_reasons: Vec::new(),
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

fn sample_semantic_scope(owner_id: &str) -> SemanticSourceScopeKeyV1 {
    SemanticSourceScopeKeyV1 {
        corpus_kind: SemanticCorpusKindV1::SymbolCard,
        owner_kind: OwnerDocKind::Symbol,
        owner_id: owner_id.to_string(),
    }
}

fn sample_semantic_source(owner_id: &str) -> SemanticSourceRecordV1 {
    SemanticSourceRecordV1 {
        record_id: format!("record-{owner_id}"),
        corpus_kind: SemanticCorpusKindV1::SymbolCard,
        owner_kind: OwnerDocKind::Symbol,
        owner_id: owner_id.to_string(),
        source_doc_id: format!("doc-{owner_id}"),
        parent_owner_id: None,
        repo_relative_path: RepoRelativePath::new("src/lib.rs"),
        language: Some("rust".to_string()),
        package: Some("crate".to_string()),
        symbol_kind: Some("function".to_string()),
        visibility: Some("pub".to_string()),
        source_role: SourceRoleV1::CardText,
        generated: false,
        capability_status: CapabilityStatusV1::Full,
        raw_fallback_reason: None,
        authority_digest: format!("authority:{owner_id}"),
        render_policy_digest: "render:v1".to_string(),
        card_schema_version: 1,
        text: format!("semantic source for {owner_id}"),
    }
}

fn sample_cluster_semantic_scope(owner_id: &str) -> SemanticSourceScopeKeyV1 {
    SemanticSourceScopeKeyV1 {
        corpus_kind: SemanticCorpusKindV1::ClusterCard,
        owner_kind: OwnerDocKind::Module,
        owner_id: owner_id.to_string(),
    }
}

fn sample_cluster_semantic_source(owner_id: &str, record_suffix: &str) -> SemanticSourceRecordV1 {
    let mut source = sample_semantic_source(owner_id);
    source.record_id = format!("cluster-record-{record_suffix}");
    source.corpus_kind = SemanticCorpusKindV1::ClusterCard;
    source.owner_kind = OwnerDocKind::Module;
    source.authority_digest = format!("cluster-authority-{record_suffix}");
    source.text = "rendered text mentions symbol:fake and is not membership authority".to_string();
    source
}

fn sample_cluster_membership(
    source: &SemanticSourceRecordV1,
    member_suffix: &str,
) -> quanta_index_contract::ClusterMembershipReplaceV1 {
    quanta_index_contract::ClusterMembershipReplaceV1 {
        cluster_record_id: source.record_id.clone(),
        authority_digest: source.authority_digest.clone(),
        members: vec![SymbolId::new(format!("symbol:member:{member_suffix}"))],
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
            window: QueryResultWindowV1::exact(0),
            file_owner_rows: None,
        },
    )))
}

fn unused_control() -> Arc<StubControlTransport> {
    Arc::new(StubControlTransport::new(
        quanta_index_contract::SearchPlaneControlIpcResponse::Error(SearchPlaneIpcError {
            code: "UNUSED_CONTROL".to_string(),
            message: "test must install an explicit control response".to_string(),
            repair: None,
        }),
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
    assert_eq!(resolved.state_root, Some(PathBuf::from("/tmp/qi-state")));
    assert_eq!(
        resolved.query_socket,
        PathBuf::from("/tmp/qi-state/search-plane/query.sock")
    );
    assert_eq!(
        resolved.control_socket,
        PathBuf::from("/tmp/qi-state/search-plane/control.sock")
    );
    assert_eq!(
        resolved.ingest_socket,
        PathBuf::from("/tmp/qi-state/search-plane/ingest.sock"),
        "QI-SDK-01: ingest socket resolves to state_root/search-plane/ingest.sock"
    );
    assert_eq!(
        resolved.io_policy,
        quanta_index_ipc::ClientIoPolicy::default()
    );
}

#[test]
fn connect_options_preserve_explicit_request_io_timeout() {
    let timeout = std::time::Duration::from_millis(125);
    let resolved = ok_or_fail!(
        ConnectOptions::from_state_root("/tmp/qi-state")
            .with_request_io_timeout(timeout)
            .resolve()
    );
    assert_eq!(resolved.io_policy.request_timeout(), timeout);
}

#[test]
fn connect_options_preserve_absolute_request_io_deadline() {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let resolved = ok_or_fail!(
        ConnectOptions::from_state_root("/tmp/qi-state")
            .with_request_io_deadline(deadline)
            .resolve()
    );
    assert_eq!(resolved.io_policy.absolute_deadline(), Some(deadline));
}

#[test]
fn connect_options_reject_elapsed_request_io_deadline() {
    let deadline = std::time::Instant::now()
        .checked_sub(std::time::Duration::from_millis(1))
        .expect("monotonic clock must represent an instant 1ms in the past");
    let result = ConnectOptions::from_state_root("/tmp/qi-state")
        .with_request_io_deadline(deadline)
        .resolve();
    assert!(
        matches!(result, Err(crate::SdkError::Usage(message)) if message.contains("deadline elapsed"))
    );
}

#[test]
fn connect_options_reject_zero_request_io_timeout() {
    let result = ConnectOptions::from_state_root("/tmp/qi-state")
        .with_request_io_timeout(std::time::Duration::ZERO)
        .resolve();
    assert!(
        matches!(result, Err(crate::SdkError::Usage(message)) if message.contains("greater than zero"))
    );
}

#[test]
fn cluster_membership_read_routes_exact_request_and_validates_available_authority_v1() {
    let request = sample_cluster_membership_request();
    let snapshot = quanta_index_contract::ClusterMembershipSnapshotV1 {
        cluster_record_id: request.cluster_record_id.clone(),
        generation: request.generation.clone(),
        authority_digest: request.expected_authority_digest.clone(),
        members: vec![
            SymbolId::new("symbol:auth::authenticate"),
            SymbolId::new("symbol:auth::authorize"),
        ],
        completeness: quanta_index_contract::ClusterMembershipCompletenessV1::Complete,
    };
    let expected = quanta_index_contract::ClusterMembershipReadOutcomeV1::Available(snapshot);
    let query = Arc::new(StubQueryTransport::new(
        SearchPlaneQueryIpcResponse::ClusterMembershipRead(
            quanta_index_contract::ClusterMembershipBatchReadResponseV1 {
                outcomes: vec![expected.clone()],
            },
        ),
    ));
    let client = QuantaIndex::from_transports(query.clone(), unused_control(), unused_ingest());

    let observed = ok_or_fail!(client.search().cluster_membership_read_v1(request.clone()));
    assert_eq!(observed, expected);
    let captured = ok_or_fail!(only_query_request(query.as_ref()));
    assert_eq!(
        captured.payload,
        quanta_index_contract::SearchPlaneQueryIpcRequest::ClusterMembershipRead(
            quanta_index_contract::ClusterMembershipBatchReadRequestV1::single_v1(request),
        )
    );
}

#[test]
fn cluster_membership_read_preserves_matching_typed_absence_and_rejection_v1() {
    let request = sample_cluster_membership_request();
    let outcomes = [
        quanta_index_contract::ClusterMembershipReadOutcomeV1::Unavailable(
            quanta_index_contract::ClusterMembershipUnavailableV1 {
                cluster_record_id: request.cluster_record_id.clone(),
                generation: request.generation.clone(),
                expected_authority_digest: request.expected_authority_digest.clone(),
            },
        ),
        quanta_index_contract::ClusterMembershipReadOutcomeV1::Rejected(
            quanta_index_contract::ClusterMembershipReadRejectionV1 {
                cluster_record_id: request.cluster_record_id.clone(),
                generation: request.generation.clone(),
                expected_authority_digest: request.expected_authority_digest.clone(),
                failure:
                    quanta_index_contract::ClusterMembershipReadFailureV1::CurrentGenerationMissing,
            },
        ),
    ];

    for expected in outcomes {
        let query = Arc::new(StubQueryTransport::new(
            SearchPlaneQueryIpcResponse::ClusterMembershipRead(
                quanta_index_contract::ClusterMembershipBatchReadResponseV1 {
                    outcomes: vec![expected.clone()],
                },
            ),
        ));
        let client = QuantaIndex::from_transports(query, unused_control(), unused_ingest());
        let observed = ok_or_fail!(client.search().cluster_membership_read_v1(request.clone()));
        assert_eq!(observed, expected);
    }
}

#[test]
fn cluster_membership_read_rejects_stale_response_authority_v1() {
    let request = sample_cluster_membership_request();
    let stale = quanta_index_contract::ClusterMembershipReadOutcomeV1::Available(
        quanta_index_contract::ClusterMembershipSnapshotV1 {
            cluster_record_id: request.cluster_record_id.clone(),
            generation: request.generation.clone(),
            authority_digest: "stale-authority-digest".to_string(),
            members: vec![SymbolId::new("symbol:auth::authenticate")],
            completeness: quanta_index_contract::ClusterMembershipCompletenessV1::Complete,
        },
    );
    let query = Arc::new(StubQueryTransport::new(
        SearchPlaneQueryIpcResponse::ClusterMembershipRead(
            quanta_index_contract::ClusterMembershipBatchReadResponseV1 {
                outcomes: vec![stale],
            },
        ),
    ));
    let client = QuantaIndex::from_transports(query.clone(), unused_control(), unused_ingest());

    let error = match client.search().cluster_membership_read_v1(request) {
        Ok(outcome) => panic!("stale membership authority unexpectedly admitted: {outcome:?}"),
        Err(error) => error,
    };
    assert!(matches!(error, crate::SdkError::Protocol(_)));
    assert_eq!(
        ok_or_fail!(query.requests.lock()).len(),
        1,
        "authority mismatch must be detected after one exact transport read"
    );
}

#[test]
fn cluster_membership_read_rejects_mismatched_absence_and_rejection_authority_v1() {
    let request = sample_cluster_membership_request();
    let mismatched_outcomes = [
        quanta_index_contract::ClusterMembershipReadOutcomeV1::Unavailable(
            quanta_index_contract::ClusterMembershipUnavailableV1 {
                cluster_record_id: "cluster-card:other".to_string(),
                generation: request.generation.clone(),
                expected_authority_digest: request.expected_authority_digest.clone(),
            },
        ),
        quanta_index_contract::ClusterMembershipReadOutcomeV1::Rejected(
            quanta_index_contract::ClusterMembershipReadRejectionV1 {
                cluster_record_id: request.cluster_record_id.clone(),
                generation: request.generation.clone(),
                expected_authority_digest: "stale-authority-digest".to_string(),
                failure:
                    quanta_index_contract::ClusterMembershipReadFailureV1::AuthorityDigestMismatch,
            },
        ),
    ];

    for mismatched in mismatched_outcomes {
        let query = Arc::new(StubQueryTransport::new(
            SearchPlaneQueryIpcResponse::ClusterMembershipRead(
                quanta_index_contract::ClusterMembershipBatchReadResponseV1 {
                    outcomes: vec![mismatched],
                },
            ),
        ));
        let client = QuantaIndex::from_transports(query, unused_control(), unused_ingest());
        let error = match client.search().cluster_membership_read_v1(request.clone()) {
            Ok(outcome) => {
                panic!("mismatched membership outcome authority unexpectedly admitted: {outcome:?}")
            }
            Err(error) => error,
        };
        assert!(matches!(error, crate::SdkError::Protocol(_)));
    }
}

#[test]
fn cluster_membership_read_rejects_invalid_request_before_transport_v1() {
    let mut request = sample_cluster_membership_request();
    request.limit = 0;
    let query = unused_query();
    let client = QuantaIndex::from_transports(query.clone(), unused_control(), unused_ingest());

    let error = match client.search().cluster_membership_read_v1(request) {
        Ok(outcome) => panic!("invalid membership request unexpectedly admitted: {outcome:?}"),
        Err(error) => error,
    };
    assert!(matches!(error, crate::SdkError::Usage(_)));
    assert!(
        ok_or_fail!(query.requests.lock()).is_empty(),
        "invalid request must not reach query transport"
    );
}

#[test]
fn cluster_membership_read_rejects_unrelated_query_response_v1() {
    let request = sample_cluster_membership_request();
    let query = unused_query();
    let client = QuantaIndex::from_transports(query, unused_control(), unused_ingest());

    let error = match client.search().cluster_membership_read_v1(request) {
        Ok(outcome) => panic!("unrelated query response unexpectedly admitted: {outcome:?}"),
        Err(error) => error,
    };
    let crate::SdkError::Protocol(message) = error else {
        panic!("expected protocol error for unrelated response");
    };
    assert_eq!(
        message,
        "expected cluster membership read response, got text"
    );
}

#[test]
fn cluster_membership_batch_read_routes_fifteen_items_once_and_preserves_order_v1() {
    let request = sample_cluster_membership_batch(15);
    let expected = sample_cluster_membership_batch_response(&request);
    let query = Arc::new(StubQueryTransport::new(
        SearchPlaneQueryIpcResponse::ClusterMembershipRead(expected.clone()),
    ));
    let client = QuantaIndex::from_transports(query.clone(), unused_control(), unused_ingest());

    let observed = ok_or_fail!(
        client
            .search()
            .cluster_membership_batch_read_v1(request.clone())
    );
    assert_eq!(observed, expected);
    let dispatched = {
        let requests = ok_or_fail!(query.requests.lock());
        assert_eq!(
            requests.len(),
            1,
            "one logical batch must use one transport call"
        );
        requests
            .first()
            .expect("transport call count was just asserted")
            .payload
            .clone()
    };
    assert_eq!(
        dispatched,
        quanta_index_contract::SearchPlaneQueryIpcRequest::ClusterMembershipRead(request)
    );
}

#[test]
fn cluster_membership_batch_read_rejects_partial_reordered_and_stale_response_v1() {
    let request = sample_cluster_membership_batch(3);
    let valid = sample_cluster_membership_batch_response(&request);

    let mut partial = valid.clone();
    let _removed = partial.outcomes.pop();

    let mut reordered = valid.clone();
    reordered.outcomes.swap(0, 1);

    let mut stale = valid;
    let quanta_index_contract::ClusterMembershipReadOutcomeV1::Available(snapshot) = stale
        .outcomes
        .get_mut(1)
        .expect("fixture batch must carry at least two outcomes")
    else {
        panic!("fixture must contain an available outcome")
    };
    snapshot.authority_digest.push_str(":stale");

    for malformed in [partial, reordered, stale] {
        let query = Arc::new(StubQueryTransport::new(
            SearchPlaneQueryIpcResponse::ClusterMembershipRead(malformed),
        ));
        let client = QuantaIndex::from_transports(query.clone(), unused_control(), unused_ingest());
        let error = client
            .search()
            .cluster_membership_batch_read_v1(request.clone())
            .expect_err("malformed batch response must fail the whole SDK call");
        assert!(matches!(error, crate::SdkError::Protocol(_)));
        assert_eq!(ok_or_fail!(query.requests.lock()).len(), 1);
    }
}

#[test]
fn semantic_query_builder_emits_active_selector_and_query_text() {
    let query = Arc::new(StubQueryTransport::new(
        SearchPlaneQueryIpcResponse::Semantic(SemanticQueryResponse {
            generation: sample_generation_pin(),
            results: vec![sample_hit()],
            window: QueryResultWindowV1::exact(1),
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
            window: QueryResultWindowV1::exact(1),
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
            window: QueryResultWindowV1::exact(1),
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
fn lexical_constraint_setters_preserve_path_and_language_axes_v1() {
    let query = Arc::new(StubQueryTransport::new(SearchPlaneQueryIpcResponse::Text(
        TextQueryResponse {
            generation: sample_generation_pin(),
            results: Vec::new(),
            window: QueryResultWindowV1::exact(0),
            file_owner_rows: None,
        },
    )));
    let client = QuantaIndex::from_transports(query.clone(), unused_control(), unused_ingest());
    let path = ExactRepoRelativePathV1::new("src/lib.rs").expect("valid exact path");
    let rust = LanguageCode::new("rust").expect("valid language");
    let _response = ok_or_fail!(
        client
            .lexical()
            .query()
            .native("needle")
            .exact_repo_relative_path(path.clone())
            .language_any_of([rust.clone()])
            .active(repo_id(), revision_id())
            .top_k(3)
            .execute()
    );
    let captured = ok_or_fail!(only_query_request(query.as_ref()));
    assert!(
        matches!(
            &captured.payload,
            quanta_index_contract::SearchPlaneQueryIpcRequest::Text(_)
        ),
        "expected text query request, got {:?}",
        captured.payload
    );
    let quanta_index_contract::SearchPlaneQueryIpcRequest::Text(request) = &captured.payload else {
        return;
    };
    assert_eq!(
        request.constraints.repo_relative_path_exact.as_ref(),
        Some(&path),
        "language setter must not erase the exact-path axis"
    );
    assert_eq!(
        request.constraints.language_any_of,
        std::collections::BTreeSet::from([rust])
    );
}

#[test]
fn semantic_hybrid_seed_and_symbol_setters_preserve_both_constraint_axes_v1() {
    let path = ExactRepoRelativePathV1::new("src/lib.rs").expect("valid exact path");
    let rust = LanguageCode::new("rust").expect("valid language");
    let expected_languages = std::collections::BTreeSet::from([rust.clone()]);

    let semantic_transport = Arc::new(StubQueryTransport::new(
        SearchPlaneQueryIpcResponse::Semantic(SemanticQueryResponse {
            generation: sample_generation_pin(),
            results: Vec::new(),
            window: QueryResultWindowV1::exact(0),
            explanation: sample_explanation(),
        }),
    ));
    let semantic_client = QuantaIndex::from_transports(
        semantic_transport.clone(),
        unused_control(),
        unused_ingest(),
    );
    let _semantic_response = ok_or_fail!(
        semantic_client
            .semantic()
            .query()
            .text("needle")
            .language_any_of([rust.clone()])
            .exact_repo_relative_path(path.clone())
            .scope_native("needle")
            .scope_top_k(3)
            .active(repo_id(), revision_id())
            .top_k(3)
            .execute()
    );
    let semantic_request = ok_or_fail!(only_query_request(semantic_transport.as_ref()));
    let quanta_index_contract::SearchPlaneQueryIpcRequest::Semantic(semantic_request) =
        &semantic_request.payload
    else {
        return;
    };
    assert_eq!(
        semantic_request
            .constraints
            .repo_relative_path_exact
            .as_ref(),
        Some(&path)
    );
    assert_eq!(
        semantic_request.constraints.language_any_of,
        expected_languages
    );
    assert_eq!(
        semantic_request
            .lexical_scope
            .as_ref()
            .map(|scope| &scope.constraints),
        Some(&semantic_request.constraints),
        "semantic scope and dense leg must share the exact same constraint authority"
    );

    let hybrid_transport = Arc::new(StubQueryTransport::new(
        SearchPlaneQueryIpcResponse::HybridSeed(HybridSeedQueryResponse {
            generation: sample_generation_pin(),
            manifest_digest: "manifest-digest".to_string(),
            seed_candidates: Vec::new(),
            window: QueryResultWindowV1::exact(0),
            explanation: sample_explanation(),
        }),
    ));
    let hybrid_client =
        QuantaIndex::from_transports(hybrid_transport.clone(), unused_control(), unused_ingest());
    let _hybrid_response = ok_or_fail!(
        hybrid_client
            .search()
            .hybrid_seed()
            .native("needle")
            .semantic_text("needle")
            .exact_repo_relative_path(path.clone())
            .language_any_of([rust.clone()])
            .active(repo_id(), revision_id())
            .top_k(3)
            .execute()
    );
    let hybrid_request = ok_or_fail!(only_query_request(hybrid_transport.as_ref()));
    let quanta_index_contract::SearchPlaneQueryIpcRequest::HybridSeed(hybrid_request) =
        &hybrid_request.payload
    else {
        return;
    };
    assert_eq!(
        hybrid_request
            .text_query
            .constraints
            .repo_relative_path_exact
            .as_ref(),
        Some(&path)
    );
    assert_eq!(
        hybrid_request.text_query.constraints.language_any_of,
        expected_languages
    );

    let symbol_transport = Arc::new(StubQueryTransport::new(
        SearchPlaneQueryIpcResponse::Symbol(quanta_index_contract::SymbolQueryResponse {
            generation: sample_generation_pin(),
            results: Vec::new(),
            window: QueryResultWindowV1::exact(0),
        }),
    ));
    let symbol_client =
        QuantaIndex::from_transports(symbol_transport.clone(), unused_control(), unused_ingest());
    let _symbol_response = ok_or_fail!(
        symbol_client
            .symbol()
            .query()
            .native("")
            .language_any_of([rust])
            .exact_repo_relative_path(path.clone())
            .active(repo_id(), revision_id())
            .top_k(3)
            .execute()
    );
    let symbol_request = ok_or_fail!(only_query_request(symbol_transport.as_ref()));
    let quanta_index_contract::SearchPlaneQueryIpcRequest::Symbol(symbol_request) =
        &symbol_request.payload
    else {
        return;
    };
    assert_eq!(
        symbol_request.constraints.repo_relative_path_exact.as_ref(),
        Some(&path)
    );
    assert_eq!(
        symbol_request.constraints.language_any_of,
        expected_languages
    );
}

#[test]
fn lexical_query_request_forwards_contract_dto_unchanged() {
    let query = Arc::new(StubQueryTransport::new(SearchPlaneQueryIpcResponse::Text(
        TextQueryResponse {
            generation: sample_generation_pin(),
            results: vec![sample_hit()],
            window: QueryResultWindowV1::exact(1),
            file_owner_rows: None,
        },
    )));
    let client = QuantaIndex::from_transports(query.clone(), unused_control(), unused_ingest());
    let request = quanta_index_contract::TextQueryRequest {
        syntax: quanta_index_contract::TextQuerySyntax::Sourcegraph,
        query_text: "repo:repo-1 lang:rust sample".to_string(),
        constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
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
            window: QueryResultWindowV1::exact(1),
        }),
    ));
    let client = QuantaIndex::from_transports(query.clone(), unused_control(), unused_ingest());
    let request = quanta_index_contract::SymbolQueryRequest {
        syntax: quanta_index_contract::TextQuerySyntax::Native,
        query_text: "symbol:sample".to_string(),
        constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
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
            window: QueryResultWindowV1::exact(1),
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
            window: QueryResultWindowV1::exact(1),
            explanation: sample_explanation(),
        }),
    ));
    let client = QuantaIndex::from_transports(query.clone(), unused_control(), unused_ingest());
    let request = quanta_index_contract::SemanticQueryRequest {
        query_text: "legacy semantic text".to_string(),
        constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
        generation: None,
        generation_selector: Some(GenerationSelector::Active {
            repo_id: repo_id(),
            revision_id: revision_id(),
        }),
        lexical_scope: Some(quanta_index_contract::TextQueryRequest {
            syntax: quanta_index_contract::TextQuerySyntax::Sourcegraph,
            query_text: "repo:repo-1 file:src/lib.rs".to_string(),
            constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
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
            window: QueryResultWindowV1::exact(1),
            explanation: sample_explanation(),
        }),
    ));
    let client = QuantaIndex::from_transports(query.clone(), unused_control(), unused_ingest());
    let request = quanta_index_contract::HybridSeedQueryRequest {
        text_query: quanta_index_contract::TextQueryRequest {
            syntax: quanta_index_contract::TextQuerySyntax::Native,
            query_text: "hybrid text".to_string(),
            constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
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
            window: QueryResultWindowV1::exact(1),
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
        manifest_digest: Some("manifest:feed".to_string()),
        batch_digest: "batch:feed".to_string(),
        applied: true,
        durable_sequence: 7,
        accepted_clear_surfaces: 0,
        accepted_replace_scopes: 1,
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
fn search_corpus_builder_preserves_semantic_lifecycle_in_canonical_wire_order() {
    let receipt = BatchPublishReceipt {
        generation: ManifestGeneration::new(1),
        manifest_digest: Some("manifest:semantic".to_string()),
        batch_digest: "batch:semantic".to_string(),
        applied: true,
        durable_sequence: 7,
        accepted_clear_surfaces: 0,
        accepted_replace_scopes: 0,
        accepted_tombstone_scopes: 0,
        sealed: true,
    };
    let ingest = Arc::new(StubIngestTransport::new(
        SearchPlaneIngestIpcResponse::SearchCorpusReceipt(receipt),
    ));
    let client = QuantaIndex::from_transports(unused_query(), unused_control(), ingest.clone());
    let scope_a = sample_semantic_scope("symbol-a");
    let scope_b = sample_semantic_scope("symbol-b");
    let tombstone_c = sample_semantic_scope("symbol-c");
    let tombstone_d = sample_semantic_scope("symbol-d");
    let batch = SearchCorpusBatch::replace_generation(
        repo_id(),
        revision_id(),
        ManifestGeneration::new(1),
        "manifest:semantic",
        "batch:semantic",
    )
    .replace_semantic_scope(
        scope_b.clone(),
        "scope:b",
        vec![sample_semantic_source("symbol-b")],
    )
    .replace_semantic_scope(
        scope_a.clone(),
        "scope:a",
        vec![sample_semantic_source("symbol-a")],
    )
    .tombstone_semantic_scope(tombstone_d.clone())
    .tombstone_semantic_scope(tombstone_c.clone());

    assert_eq!(
        batch
            .semantic_replace_scopes()
            .iter()
            .map(|mutation| mutation.scope.clone())
            .collect::<Vec<_>>(),
        vec![scope_a, scope_b]
    );
    assert_eq!(
        batch.semantic_tombstone_scopes(),
        &[tombstone_c, tombstone_d]
    );

    let _receipt = ok_or_fail!(client.search_corpus().publish(&batch));
    let captured = ok_or_fail!(only_ingest_request(ingest.as_ref()));
    let SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(wire) = captured.payload else {
        panic!("expected search corpus wire batch");
    };
    assert_eq!(
        wire.semantic_replace_scopes,
        batch.semantic_replace_scopes()
    );
    assert_eq!(
        wire.semantic_tombstone_scopes,
        batch.semantic_tombstone_scopes()
    );

    let unsealed = batch.without_seal();
    assert!(!unsealed.seal_requested());
    assert_eq!(unsealed.semantic_replace_scopes().len(), 2);
    assert_eq!(unsealed.semantic_tombstone_scopes().len(), 2);
}

#[test]
fn search_corpus_builder_preserves_typed_cluster_membership_without_text_inference_v1() {
    let receipt = BatchPublishReceipt {
        generation: ManifestGeneration::new(1),
        manifest_digest: Some("manifest:cluster".to_string()),
        batch_digest: "batch:cluster".to_string(),
        applied: true,
        durable_sequence: 7,
        accepted_clear_surfaces: 0,
        accepted_replace_scopes: 0,
        accepted_tombstone_scopes: 0,
        sealed: true,
    };
    let ingest = Arc::new(StubIngestTransport::new(
        SearchPlaneIngestIpcResponse::SearchCorpusReceipt(receipt),
    ));
    let client = QuantaIndex::from_transports(unused_query(), unused_control(), ingest.clone());
    let source_a = sample_cluster_semantic_source("auth-service", "a");
    let source_b = sample_cluster_semantic_source("auth-service", "b");
    let membership_a = sample_cluster_membership(&source_a, "a");
    let membership_b = sample_cluster_membership(&source_b, "b");
    let batch = SearchCorpusBatch::replace_generation(
        repo_id(),
        revision_id(),
        ManifestGeneration::new(1),
        "manifest:cluster",
        "batch:cluster",
    )
    .replace_semantic_scope_with_cluster_memberships_v1(
        sample_cluster_semantic_scope("auth-service"),
        "scope:cluster",
        vec![source_b, source_a],
        vec![membership_b, membership_a.clone()],
    );

    let _receipt = ok_or_fail!(client.search_corpus().publish(&batch));
    let captured = ok_or_fail!(only_ingest_request(ingest.as_ref()));
    let SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(wire) = captured.payload else {
        panic!("expected search corpus wire batch");
    };
    let Some(scope) = wire.semantic_replace_scopes.first() else {
        panic!("expected one semantic replace scope");
    };
    assert_eq!(scope.cluster_memberships.len(), 2);
    assert_eq!(scope.cluster_memberships.first(), Some(&membership_a));
    assert!(
        scope
            .cluster_memberships
            .iter()
            .flat_map(|membership| membership.members.iter())
            .all(|member| member.as_str() != "symbol:fake"),
        "rendered source text must never synthesize structured membership"
    );
}

#[test]
fn search_corpus_builder_rejects_missing_mismatched_or_misplaced_cluster_membership_v1() {
    let ingest = Arc::new(StubIngestTransport::new(
        SearchPlaneIngestIpcResponse::SearchCorpusReceipt(BatchPublishReceipt::default()),
    ));
    let client = QuantaIndex::from_transports(unused_query(), unused_control(), ingest.clone());
    let cluster_source = sample_cluster_semantic_source("auth-service", "a");

    let missing = SearchCorpusBatch::replace_generation(
        repo_id(),
        revision_id(),
        ManifestGeneration::new(1),
        "manifest:cluster-missing",
        "batch:cluster-missing",
    )
    .replace_semantic_scope(
        sample_cluster_semantic_scope("auth-service"),
        "scope:cluster-missing",
        vec![cluster_source.clone()],
    );
    let error = client
        .search_corpus()
        .publish(&missing)
        .expect_err("ClusterCard without typed membership must fail closed");
    assert!(error.to_string().contains("requires one typed membership"));

    let mut stale_membership = sample_cluster_membership(&cluster_source, "a");
    stale_membership.authority_digest = "stale-authority".to_string();
    let mismatched = SearchCorpusBatch::replace_generation(
        repo_id(),
        revision_id(),
        ManifestGeneration::new(1),
        "manifest:cluster-mismatch",
        "batch:cluster-mismatch",
    )
    .replace_semantic_scope_with_cluster_memberships_v1(
        sample_cluster_semantic_scope("auth-service"),
        "scope:cluster-mismatch",
        vec![cluster_source],
        vec![stale_membership],
    );
    let error = client
        .search_corpus()
        .publish(&mismatched)
        .expect_err("stale typed membership authority must fail closed");
    assert!(error.to_string().contains("digest does not match"));

    let symbol_source = sample_semantic_source("symbol-a");
    let misplaced = SearchCorpusBatch::replace_generation(
        repo_id(),
        revision_id(),
        ManifestGeneration::new(1),
        "manifest:membership-misplaced",
        "batch:membership-misplaced",
    )
    .replace_semantic_scope_with_cluster_memberships_v1(
        sample_semantic_scope("symbol-a"),
        "scope:membership-misplaced",
        vec![symbol_source.clone()],
        vec![sample_cluster_membership(&symbol_source, "misplaced")],
    );
    let error = client
        .search_corpus()
        .publish(&misplaced)
        .expect_err("non-ClusterCard scope with typed membership must fail closed");
    assert!(
        error
            .to_string()
            .contains("must not carry cluster membership")
    );

    assert!(
        ok_or_fail!(ingest.requests.lock()).is_empty(),
        "invalid cluster membership authority must be rejected before transport"
    );
}

#[test]
fn search_corpus_semantic_surface_conflict_fails_before_transport_io() {
    let ingest = Arc::new(StubIngestTransport::new(
        SearchPlaneIngestIpcResponse::SearchCorpusReceipt(BatchPublishReceipt::default()),
    ));
    let client = QuantaIndex::from_transports(unused_query(), unused_control(), ingest.clone());
    let batch = SearchCorpusBatch::replace_generation(
        repo_id(),
        revision_id(),
        ManifestGeneration::new(1),
        "manifest:conflict",
        "batch:conflict",
    )
    .clear_surface(SearchScopeSurface::Symbol)
    .replace_semantic_scope(
        sample_semantic_scope("symbol-conflict"),
        "scope:conflict",
        vec![sample_semantic_source("symbol-conflict")],
    );

    let error = client
        .search_corpus()
        .publish(&batch)
        .expect_err("clear plus semantic replace must fail closed");
    assert!(error.to_string().contains("cannot be cleared and replaced"));
    let requests_are_empty = {
        let requests = ingest
            .requests
            .lock()
            .expect("ingest request list should remain readable");
        requests.is_empty()
    };
    assert!(requests_are_empty, "invalid batch must not reach transport");
}

#[test]
fn search_corpus_semantic_scope_conflicts_fail_before_transport_io() {
    let ingest = Arc::new(StubIngestTransport::new(
        SearchPlaneIngestIpcResponse::SearchCorpusReceipt(BatchPublishReceipt::default()),
    ));
    let client = QuantaIndex::from_transports(unused_query(), unused_control(), ingest.clone());
    let scope = sample_semantic_scope("symbol-conflict");
    let replace_and_tombstone = SearchCorpusBatch::replace_generation(
        repo_id(),
        revision_id(),
        ManifestGeneration::new(1),
        "manifest:scope-conflict",
        "batch:scope-conflict",
    )
    .replace_semantic_scope(
        scope.clone(),
        "scope:conflict",
        vec![sample_semantic_source("symbol-conflict")],
    )
    .tombstone_semantic_scope(scope.clone());

    let error = client
        .search_corpus()
        .publish(&replace_and_tombstone)
        .expect_err("same semantic scope replace plus tombstone must fail closed");
    assert!(
        error
            .to_string()
            .contains("cannot be replaced and tombstoned")
    );

    let duplicate_replace = SearchCorpusBatch::replace_generation(
        repo_id(),
        revision_id(),
        ManifestGeneration::new(1),
        "manifest:duplicate-scope",
        "batch:duplicate-scope",
    )
    .replace_semantic_scope(
        scope.clone(),
        "scope:first",
        vec![sample_semantic_source("symbol-conflict")],
    )
    .replace_semantic_scope(
        scope,
        "scope:second",
        vec![sample_semantic_source("symbol-conflict")],
    );
    let error = client
        .search_corpus()
        .publish(&duplicate_replace)
        .expect_err("duplicate semantic replace scope must fail closed");
    assert!(error.to_string().contains("duplicate replace scope"));

    let requests_are_empty = ingest
        .requests
        .lock()
        .expect("ingest request list should remain readable")
        .is_empty();
    assert!(
        requests_are_empty,
        "invalid batches must not reach transport"
    );
}

#[test]
fn reader_client_routes_lexical_query_surface() {
    let query = Arc::new(StubQueryTransport::new(SearchPlaneQueryIpcResponse::Text(
        TextQueryResponse {
            generation: sample_generation_pin(),
            results: vec![sample_hit()],
            window: QueryResultWindowV1::exact(1),
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
        SearchPlaneIngestIpcResponse::SearchCorpusReceipt(BatchPublishReceipt {
            generation: ManifestGeneration::new(2),
            manifest_digest: Some("manifest:unsealed".to_string()),
            batch_digest: "batch:unsealed".to_string(),
            applied: true,
            durable_sequence: 7,
            accepted_clear_surfaces: 0,
            accepted_replace_scopes: 0,
            accepted_tombstone_scopes: 0,
            sealed: false,
        }),
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
        manifest_digest: Some("manifest:activate".to_string()),
        batch_digest: "batch:activate".to_string(),
        applied: true,
        durable_sequence: 7,
        accepted_clear_surfaces: 0,
        accepted_replace_scopes: 0,
        accepted_tombstone_scopes: 0,
        sealed: true,
    };
    let manifest_digest = receipt
        .manifest_digest
        .clone()
        .expect("search corpus receipt carries its manifest digest");
    let active = search_corpus_identity(receipt.generation.get(), &manifest_digest);
    let ack = SearchPlaneSearchCorpusActivationCasAck {
        active: active.clone(),
        previous_sealed_active: None,
    };
    let control = Arc::new(StubControlTransport::new(
        quanta_index_contract::SearchPlaneControlIpcResponse::SearchCorpusActivationCasAck(
            ack.clone(),
        ),
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
    let (observed_receipt, observed_ack) = ok_or_fail!(
        client
            .producer()
            .publish_search_corpus_and_activate(&batch, None)
    );
    assert_eq!(observed_receipt, receipt);
    assert_eq!(observed_ack, ack);

    let ingest_request = ok_or_fail!(only_ingest_request(ingest.as_ref()));
    assert!(matches!(
        ingest_request.payload,
        SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(_)
    ));
    let control_request = ok_or_fail!(only_control_request(control.as_ref()));
    let quanta_index_contract::SearchPlaneControlIpcRequest::ActivateSearchCorpusGenerationCas(
        request,
    ) = &control_request.payload
    else {
        panic!("expected composite search corpus activation CAS request");
    };
    assert_eq!(request.candidate, active);
    assert_eq!(request.expected_active, None);
}

#[test]
fn producer_client_rejects_activation_ack_identity_mismatches_v1() {
    let candidate = search_corpus_identity(7, "manifest:activate");
    let previous = search_corpus_identity(6, "manifest:previous");
    let mut wrong_repo = candidate.clone();
    wrong_repo.lexical.repo_id = RepoId::new("other-repo");
    let mut wrong_revision = candidate.clone();
    wrong_revision.semantic.revision_id = RevisionId::new("other-revision");
    let wrong_generation = search_corpus_identity(8, "manifest:activate");
    let wrong_digest = search_corpus_identity(7, "manifest:other");
    let cases = [
        (
            "active repo",
            SearchPlaneSearchCorpusActivationCasAck {
                active: wrong_repo,
                previous_sealed_active: Some(previous.clone()),
            },
        ),
        (
            "active revision",
            SearchPlaneSearchCorpusActivationCasAck {
                active: wrong_revision,
                previous_sealed_active: Some(previous.clone()),
            },
        ),
        (
            "active generation",
            SearchPlaneSearchCorpusActivationCasAck {
                active: wrong_generation,
                previous_sealed_active: Some(previous.clone()),
            },
        ),
        (
            "active digest",
            SearchPlaneSearchCorpusActivationCasAck {
                active: wrong_digest,
                previous_sealed_active: Some(previous.clone()),
            },
        ),
        (
            "missing previous",
            SearchPlaneSearchCorpusActivationCasAck {
                active: candidate.clone(),
                previous_sealed_active: None,
            },
        ),
        (
            "wrong previous",
            SearchPlaneSearchCorpusActivationCasAck {
                active: candidate,
                previous_sealed_active: Some(search_corpus_identity(5, "manifest:older")),
            },
        ),
    ];

    for (label, ack) in cases {
        let control = Arc::new(StubControlTransport::new(
            quanta_index_contract::SearchPlaneControlIpcResponse::SearchCorpusActivationCasAck(ack),
        ));
        let ingest = Arc::new(StubIngestTransport::new(
            SearchPlaneIngestIpcResponse::SearchCorpusReceipt(BatchPublishReceipt {
                generation: ManifestGeneration::new(7),
                manifest_digest: Some("manifest:activate".to_string()),
                batch_digest: "batch:activate".to_string(),
                applied: true,
                durable_sequence: 7,
                accepted_clear_surfaces: 0,
                accepted_replace_scopes: 0,
                accepted_tombstone_scopes: 0,
                sealed: true,
            }),
        ));
        let client = QuantaIndex::from_transports(unused_query(), control, ingest);
        let batch = SearchCorpusBatch::replace_generation(
            repo_id(),
            revision_id(),
            ManifestGeneration::new(7),
            "manifest:activate",
            "batch:activate",
        );
        let error = client
            .producer()
            .publish_search_corpus_and_activate(&batch, Some(previous.clone()))
            .expect_err(label);
        assert!(
            matches!(error, crate::SdkError::Protocol(ref message) if message.contains("acknowledgement")),
            "{label} mismatch must fail as a protocol error, got {error:?}"
        );
    }
}

#[test]
fn producer_client_rejects_mismatched_sealed_receipt_before_composite_activation_v1() {
    let receipt = BatchPublishReceipt {
        generation: ManifestGeneration::new(7),
        manifest_digest: Some("manifest:unexpected".to_string()),
        batch_digest: "batch:unexpected".to_string(),
        applied: true,
        durable_sequence: 7,
        accepted_clear_surfaces: 0,
        accepted_replace_scopes: 0,
        accepted_tombstone_scopes: 0,
        sealed: true,
    };
    let control = unused_control();
    let ingest = Arc::new(StubIngestTransport::new(
        SearchPlaneIngestIpcResponse::SearchCorpusReceipt(receipt),
    ));
    let client = QuantaIndex::from_transports(unused_query(), control.clone(), ingest);
    let batch = SearchCorpusBatch::replace_generation(
        repo_id(),
        revision_id(),
        ManifestGeneration::new(7),
        "manifest:expected",
        "batch:activate",
    );
    let error = client
        .producer()
        .publish_search_corpus_and_activate(&batch, None)
        .expect_err("mismatched sealed receipt must not be activated");
    assert!(
        matches!(error, crate::SdkError::Protocol(ref message) if message.contains("manifest digest differs")),
        "expected fail-closed receipt-integrity error, got {error:?}"
    );
    assert!(
        control
            .requests
            .lock()
            .expect("control request mutex")
            .is_empty(),
        "receipt mismatch must not emit a composite activation request"
    );
}

#[test]
fn producer_client_rejects_each_search_corpus_receipt_mismatch_before_activation_v1() {
    let batch = SearchCorpusBatch::replace_generation(
        repo_id(),
        revision_id(),
        ManifestGeneration::new(7),
        "manifest:receipt-exact",
        "batch:receipt-exact",
    )
    .replace_scope(
        sample_search_scope(),
        "scope:replace",
        vec![sample_chunk()],
        vec![sample_symbol()],
    )
    .tombstone_scope(SearchScopeKey {
        doc_surface: SearchScopeSurface::Symbol,
        repo_relative_path: RepoRelativePath::new("src/tombstone.rs"),
    });
    let valid = BatchPublishReceipt {
        generation: ManifestGeneration::new(7),
        manifest_digest: Some("manifest:receipt-exact".to_string()),
        batch_digest: "batch:receipt-exact".to_string(),
        applied: true,
        durable_sequence: 7,
        accepted_clear_surfaces: 0,
        accepted_replace_scopes: 1,
        accepted_tombstone_scopes: 1,
        sealed: true,
    };
    let mut cases = Vec::new();

    let mut wrong_generation = valid.clone();
    wrong_generation.generation = ManifestGeneration::new(8);
    cases.push(("generation", wrong_generation));

    let mut wrong_digest = valid.clone();
    wrong_digest.manifest_digest = Some("manifest:other".to_string());
    cases.push(("manifest digest", wrong_digest));

    let mut wrong_batch_digest = valid.clone();
    wrong_batch_digest.batch_digest = "batch:other".to_string();
    cases.push(("batch digest", wrong_batch_digest));

    let mut wrong_seal = valid.clone();
    wrong_seal.sealed = false;
    cases.push(("seal", wrong_seal));

    let mut wrong_replace_count = valid.clone();
    wrong_replace_count.accepted_replace_scopes = 0;
    cases.push(("replace scope", wrong_replace_count));

    let mut wrong_tombstone_count = valid.clone();
    wrong_tombstone_count.accepted_tombstone_scopes = 0;
    cases.push(("tombstone scope", wrong_tombstone_count));

    let mut wrong_clear_count = valid;
    wrong_clear_count.accepted_clear_surfaces = 1;
    cases.push(("clear surface", wrong_clear_count));

    for (label, receipt) in cases {
        let control = unused_control();
        let ingest = Arc::new(StubIngestTransport::new(
            SearchPlaneIngestIpcResponse::SearchCorpusReceipt(receipt),
        ));
        let client = QuantaIndex::from_transports(unused_query(), control.clone(), ingest);
        let error = client
            .producer()
            .publish_search_corpus_and_activate(&batch, None)
            .expect_err(label);
        assert!(
            matches!(error, crate::SdkError::Protocol(ref message) if message.contains(label)),
            "{label} mismatch must be a protocol error, got {error:?}"
        );
        assert!(
            control
                .requests
                .lock()
                .expect("control request mutex")
                .is_empty(),
            "{label} mismatch emitted a control request"
        );
    }
}

#[test]
fn producer_client_rejects_invalid_expected_composite_before_ingest_v1() {
    let invalid_expected = SearchCorpusGenerationIdentityV1 {
        lexical: GenerationSnapshot {
            repo_id: repo_id(),
            revision_id: revision_id(),
            track: Track::Lexical,
            manifest_generation: ManifestGeneration::new(6),
            manifest_digest: "manifest:6".to_string(),
        },
        semantic: GenerationSnapshot {
            repo_id: repo_id(),
            revision_id: revision_id(),
            track: Track::Lexical,
            manifest_generation: ManifestGeneration::new(6),
            manifest_digest: "manifest:6".to_string(),
        },
    };
    let ingest = unused_ingest();
    let client = QuantaIndex::from_transports(unused_query(), unused_control(), ingest.clone());
    let batch = SearchCorpusBatch::replace_generation(
        repo_id(),
        revision_id(),
        ManifestGeneration::new(7),
        "manifest:7",
        "batch:activate",
    );
    let error = client
        .producer()
        .publish_search_corpus_and_activate(&batch, Some(invalid_expected))
        .expect_err("lexical-only expected identity must be rejected before ingest");
    assert!(
        matches!(error, crate::SdkError::Protocol(ref message) if message.contains("SEMANTIC_TRACK_REQUIRED")),
        "expected typed composite-identity error, got {error:?}"
    );
    assert!(
        ingest
            .requests
            .lock()
            .expect("ingest request mutex")
            .is_empty(),
        "invalid expected identity must not seal or publish a batch"
    );
}

#[test]
fn producer_client_delegates_non_advancing_activation_rejection_before_ingest_v1() {
    let ingest = unused_ingest();
    let client = QuantaIndex::from_transports(unused_query(), unused_control(), ingest.clone());
    let batch = SearchCorpusBatch::replace_generation(
        repo_id(),
        revision_id(),
        ManifestGeneration::new(7),
        "manifest:7-new",
        "batch:activate",
    );
    let error = client
        .producer()
        .publish_search_corpus_and_activate(
            &batch,
            Some(search_corpus_identity(7, "manifest:7-current")),
        )
        .expect_err("activation candidate must strictly advance the expected active generation");
    assert!(
        matches!(error, crate::SdkError::Protocol(ref message) if message.contains("CANDIDATE_GENERATION_MUST_ADVANCE_EXPECTED_ACTIVE")),
        "expected contract-owned generation relation error, got {error:?}"
    );
    assert!(
        ingest
            .requests
            .lock()
            .expect("ingest request mutex")
            .is_empty(),
        "non-advancing activation reached ingest"
    );
}

#[test]
fn history_publish_routes_through_ingest_transport_and_carries_typed_authority_records() {
    let receipt = BatchPublishReceipt {
        generation: ManifestGeneration::new(3),
        manifest_digest: None,
        batch_digest: "batch:history".to_string(),
        applied: true,
        durable_sequence: 3,
        accepted_clear_surfaces: 0,
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
        manifest_digest: None,
        batch_digest: "batch:repo-commit-recency-3".to_string(),
        applied: true,
        durable_sequence: 7,
        accepted_clear_surfaces: 0,
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
        manifest_digest: None,
        batch_digest: "batch:repo-meta-4".to_string(),
        applied: true,
        durable_sequence: 7,
        accepted_clear_surfaces: 0,
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
        manifest_digest: None,
        batch_digest: "batch:repo-topic-5".to_string(),
        applied: true,
        durable_sequence: 7,
        accepted_clear_surfaces: 0,
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
        manifest_digest: None,
        batch_digest: "batch:file-ownership-5".to_string(),
        applied: true,
        durable_sequence: 7,
        accepted_clear_surfaces: 0,
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
        manifest_digest: None,
        batch_digest: "batch:file-contributor-6".to_string(),
        applied: true,
        durable_sequence: 7,
        accepted_clear_surfaces: 0,
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
            window: quanta_index_contract::QueryResultWindowV1::exact(0),
            examined: 0,
            next_cursor: None,
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
            window: quanta_index_contract::QueryResultWindowV1::exact(0),
            examined: 0,
            next_cursor: None,
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
            window: quanta_index_contract::QueryResultWindowV1::exact(0),
            examined: 0,
            next_cursor: None,
        }),
    ));
    let client = QuantaIndex::from_transports(query.clone(), unused_control(), unused_ingest());
    let request = HistoryQueryRequest {
        text_query: quanta_index_contract::TextQueryRequest {
            syntax: quanta_index_contract::TextQuerySyntax::Sourcegraph,
            query_text: "type:commit author:alice".to_string(),
            constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
            generation: None,
            generation_selector: Some(GenerationSelector::Active {
                repo_id: repo_id(),
                revision_id: revision_id(),
            }),
            top_k: 5,
        },
        cursor: None,
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
            window: QueryResultWindowV1::exact(1),
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
            window: QueryResultWindowV1::exact(1),
        }),
    ));
    let client = QuantaIndex::from_transports(query.clone(), unused_control(), unused_ingest());
    let request = RuntimeMetadataQueryRequest {
        text_query: quanta_index_contract::TextQueryRequest {
            syntax: quanta_index_contract::TextQuerySyntax::Native,
            query_text: "dirty:yes".to_string(),
            constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
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
            window: QueryResultWindowV1::exact(0),
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
            window: QueryResultWindowV1::exact(0),
        }),
    ));
    let client = QuantaIndex::from_transports(query.clone(), unused_control(), unused_ingest());
    let request = StructuralQueryRequest {
        text_query: quanta_index_contract::TextQueryRequest {
            syntax: quanta_index_contract::TextQuerySyntax::Sourcegraph,
            query_text: r#"patterntype:structural "function_item""#.to_string(),
            constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
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
            window: QueryResultWindowV1::exact(0),
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
            window: QueryResultWindowV1::exact(0),
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
fn generations_rollback_emits_and_accepts_only_exact_composite_ack_v1() {
    let expected_active = search_corpus_identity(11, "manifest:11");
    let target = search_corpus_identity(10, "manifest:10");
    let ack = SearchPlaneSearchCorpusRollbackCasAck {
        active: target.clone(),
        previous_sealed_active: expected_active.clone(),
    };
    let control = Arc::new(StubControlTransport::new(
        quanta_index_contract::SearchPlaneControlIpcResponse::SearchCorpusRollbackCasAck(
            ack.clone(),
        ),
    ));
    let client = QuantaIndex::from_transports(unused_query(), control.clone(), unused_ingest());
    let observed = ok_or_fail!(client.generations().rollback(
        SearchPlaneRollbackSearchCorpusGenerationCasRequest {
            expected_active: expected_active.clone(),
            target: target.clone(),
        },
    ));
    assert_eq!(observed, ack);

    let captured = ok_or_fail!(only_control_request(control.as_ref()));
    let quanta_index_contract::SearchPlaneControlIpcRequest::RollbackSearchCorpusGenerationCas(
        request,
    ) = captured.payload
    else {
        panic!("expected composite search-corpus rollback CAS request");
    };
    assert_eq!(request.expected_active, expected_active);
    assert_eq!(request.target, target);
}

#[test]
fn generations_rollback_rejects_ack_identity_mismatches_v1() {
    let expected_active = search_corpus_identity(11, "manifest:11");
    let target = search_corpus_identity(10, "manifest:10");
    let cases = [
        (
            "active",
            SearchPlaneSearchCorpusRollbackCasAck {
                active: search_corpus_identity(9, "manifest:9"),
                previous_sealed_active: expected_active.clone(),
            },
        ),
        (
            "previous",
            SearchPlaneSearchCorpusRollbackCasAck {
                active: target.clone(),
                previous_sealed_active: search_corpus_identity(12, "manifest:12"),
            },
        ),
    ];
    for (label, ack) in cases {
        let control = Arc::new(StubControlTransport::new(
            quanta_index_contract::SearchPlaneControlIpcResponse::SearchCorpusRollbackCasAck(ack),
        ));
        let client = QuantaIndex::from_transports(unused_query(), control, unused_ingest());
        let error = client
            .generations()
            .rollback(SearchPlaneRollbackSearchCorpusGenerationCasRequest {
                expected_active: expected_active.clone(),
                target: target.clone(),
            })
            .expect_err(label);
        assert!(
            matches!(error, crate::SdkError::Protocol(ref message) if message.contains("acknowledgement")),
            "{label} mismatch must fail as a protocol error, got {error:?}"
        );
    }
}

#[test]
fn generations_rollback_rejects_invalid_composite_request_before_transport_v1() {
    let base_ack = SearchPlaneSearchCorpusRollbackCasAck {
        active: search_corpus_identity(10, "manifest:10"),
        previous_sealed_active: search_corpus_identity(11, "manifest:11"),
    };
    let mut malformed_target = search_corpus_identity(10, "manifest:10");
    malformed_target.semantic.track = Track::Lexical;
    let mut other_repo_target = search_corpus_identity(10, "manifest:10");
    other_repo_target.lexical.repo_id = RepoId::new("other-repo");
    other_repo_target.semantic.repo_id = RepoId::new("other-repo");
    let requests = [
        SearchPlaneRollbackSearchCorpusGenerationCasRequest {
            expected_active: search_corpus_identity(11, "manifest:11"),
            target: malformed_target,
        },
        SearchPlaneRollbackSearchCorpusGenerationCasRequest {
            expected_active: search_corpus_identity(11, "manifest:11"),
            target: other_repo_target,
        },
        SearchPlaneRollbackSearchCorpusGenerationCasRequest {
            expected_active: search_corpus_identity(11, "manifest:11"),
            target: search_corpus_identity(11, "manifest:same-generation"),
        },
    ];

    for request in requests {
        let control = Arc::new(StubControlTransport::new(
            quanta_index_contract::SearchPlaneControlIpcResponse::SearchCorpusRollbackCasAck(
                base_ack.clone(),
            ),
        ));
        let client = QuantaIndex::from_transports(unused_query(), control.clone(), unused_ingest());
        let error = client
            .generations()
            .rollback(request)
            .expect_err("invalid composite rollback request must fail before transport");
        assert!(matches!(error, crate::SdkError::Protocol(_)));
        assert!(
            control
                .requests
                .lock()
                .expect("control request mutex")
                .is_empty(),
            "invalid composite rollback request reached the control transport"
        );
    }
}

#[test]
fn control_request_id_mismatch_is_rejected_for_activation_and_rollback_v1() {
    let candidate = search_corpus_identity(7, "manifest:request-id");
    let activation_control = Arc::new(StubControlTransport::with_request_id_offset(
        quanta_index_contract::SearchPlaneControlIpcResponse::SearchCorpusActivationCasAck(
            SearchPlaneSearchCorpusActivationCasAck {
                active: candidate,
                previous_sealed_active: None,
            },
        ),
        1,
    ));
    let ingest = Arc::new(StubIngestTransport::new(
        SearchPlaneIngestIpcResponse::SearchCorpusReceipt(BatchPublishReceipt {
            generation: ManifestGeneration::new(7),
            manifest_digest: Some("manifest:request-id".to_string()),
            batch_digest: "batch:request-id".to_string(),
            applied: true,
            durable_sequence: 7,
            accepted_clear_surfaces: 0,
            accepted_replace_scopes: 0,
            accepted_tombstone_scopes: 0,
            sealed: true,
        }),
    ));
    let client = QuantaIndex::from_transports(unused_query(), activation_control.clone(), ingest);
    let batch = SearchCorpusBatch::replace_generation(
        repo_id(),
        revision_id(),
        ManifestGeneration::new(7),
        "manifest:request-id",
        "batch:request-id",
    );
    let activation_error = client
        .producer()
        .publish_search_corpus_and_activate(&batch, None)
        .expect_err("activation response with a different request id must fail");
    assert!(
        matches!(activation_error, crate::SdkError::Protocol(ref message) if message.contains("control response request_id")),
        "activation request-id mismatch must be a protocol error, got {activation_error:?}"
    );
    assert_eq!(
        activation_control
            .requests
            .lock()
            .expect("activation control request mutex")
            .len(),
        1
    );

    let expected_active = search_corpus_identity(11, "manifest:11");
    let target = search_corpus_identity(10, "manifest:10");
    let rollback_control = Arc::new(StubControlTransport::with_request_id_offset(
        quanta_index_contract::SearchPlaneControlIpcResponse::SearchCorpusRollbackCasAck(
            SearchPlaneSearchCorpusRollbackCasAck {
                active: target.clone(),
                previous_sealed_active: expected_active.clone(),
            },
        ),
        1,
    ));
    let client =
        QuantaIndex::from_transports(unused_query(), rollback_control.clone(), unused_ingest());
    let rollback_error = client
        .generations()
        .rollback(SearchPlaneRollbackSearchCorpusGenerationCasRequest {
            expected_active,
            target,
        })
        .expect_err("rollback response with a different request id must fail");
    assert!(
        matches!(rollback_error, crate::SdkError::Protocol(ref message) if message.contains("control response request_id")),
        "rollback request-id mismatch must be a protocol error, got {rollback_error:?}"
    );
    assert_eq!(
        rollback_control
            .requests
            .lock()
            .expect("rollback control request mutex")
            .len(),
        1
    );
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

/// QI-BB-015: the metrics scrape rides the control socket and comes back
/// as the typed snapshot, exactly as the daemon encoded it.
#[test]
fn observability_metrics_snapshot_returns_the_daemon_snapshot() {
    use quanta_index_contract::{
        MetricBucketV1, MetricCounterV1, MetricGaugeV1, MetricHistogramV1, MetricsDiagnosticsV1,
        MetricsSnapshotV1, SearchPlaneControlIpcRequest, SearchPlaneControlIpcResponse,
    };
    let snapshot = MetricsSnapshotV1 {
        counters: vec![MetricCounterV1 {
            name: "lq_query_intake_total".to_string(),
            value: 12,
        }],
        gauges: vec![MetricGaugeV1 {
            name: "ipc_query_connections_live".to_string(),
            value: 1.0,
        }],
        histograms: vec![MetricHistogramV1 {
            name: "lq_route_lexical_latency_ms".to_string(),
            count: 2,
            sum: 7.0,
            min: 3.0,
            max: 4.0,
            buckets: vec![MetricBucketV1 { le: 5.0, count: 2 }],
        }],
        diagnostics: MetricsDiagnosticsV1 {
            samples_recorded: 14,
            samples_dropped: 0,
            errors_recorded: 0,
            errors_dropped: 0,
        },
    };
    let control = Arc::new(StubControlTransport::new(
        SearchPlaneControlIpcResponse::MetricsSnapshot(snapshot.clone()),
    ));
    let client = QuantaIndex::from_transports(unused_query(), control.clone(), unused_ingest());
    let observed = ok_or_fail!(client.observability().metrics_snapshot());
    assert_eq!(observed, snapshot);
    let sent: Vec<SearchPlaneControlIpcRequestEnvelope> = ok_or_fail!(
        control
            .requests
            .lock()
            .map(|requests| requests.clone())
            .map_err(|err| crate::SdkError::Protocol(err.to_string()))
    );
    assert_eq!(sent.len(), 1);
    assert!(
        matches!(
            sent.first().map(|request| &request.payload),
            Some(SearchPlaneControlIpcRequest::MetricsSnapshot(_))
        ),
        "the scrape request is what went over the wire: {sent:?}"
    );
    // The control client is the same call under its own name.
    let control = Arc::new(StubControlTransport::new(
        SearchPlaneControlIpcResponse::MetricsSnapshot(snapshot.clone()),
    ));
    let client = QuantaIndex::from_transports(unused_query(), control, unused_ingest());
    let via_control = ok_or_fail!(client.control().metrics_snapshot());
    assert_eq!(via_control, snapshot);
}

/// QI-BB-015: a control answer of the wrong kind is a protocol error, and a
/// typed daemon refusal surfaces as the remote error it is.
#[test]
fn observability_metrics_snapshot_refuses_wrong_kind_and_surfaces_remote_errors() {
    use quanta_index_contract::{
        GenerationStatusReport, SearchPlaneControlIpcResponse, SearchPlaneIpcError,
    };
    let wrong_kind = Arc::new(StubControlTransport::new(
        SearchPlaneControlIpcResponse::GenerationStatusReport(GenerationStatusReport {
            repo_id: repo_id(),
            revision_id: revision_id(),
            tracks: vec![],
        }),
    ));
    let client = QuantaIndex::from_transports(unused_query(), wrong_kind, unused_ingest());
    match client.observability().metrics_snapshot() {
        Err(crate::SdkError::Protocol(message)) => {
            assert!(
                message.contains("generation_status_report"),
                "the protocol error names what arrived: {message}"
            );
        }
        other => panic!("expected a protocol error, got {other:?}"),
    }
    let refused = Arc::new(StubControlTransport::new(
        SearchPlaneControlIpcResponse::Error(SearchPlaneIpcError {
            code: "METRICS_SOURCE_DEFECT".to_string(),
            message: "metrics scrape: source point name `Bad` is not [a-z][a-z0-9_]*".to_string(),
            repair: None,
        }),
    ));
    let client = QuantaIndex::from_transports(unused_query(), refused, unused_ingest());
    match client.observability().metrics_snapshot() {
        Err(crate::SdkError::Remote { code, message, .. }) => {
            assert_eq!(code, "METRICS_SOURCE_DEFECT");
            assert!(message.contains("`Bad`"), "{message}");
        }
        other => panic!("expected the daemon's typed refusal, got {other:?}"),
    }
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

// QI-BB-025: the builder refuses an out-of-range `top_k` locally, under the same
// code the daemon answers with, and never puts the request on the wire.
#[test]
fn text_query_builder_refuses_out_of_range_top_k_before_any_round_trip() {
    for top_k in [0, quanta_index_contract::PUBLIC_TOP_K_MAX + 1, u32::MAX] {
        let query = unused_query();
        let client = QuantaIndex::from_transports(query.clone(), unused_control(), unused_ingest());
        let outcome = client
            .lexical()
            .query()
            .native("needle")
            .active(RepoId::new("repo"), RevisionId::new("rev"))
            .top_k(top_k)
            .execute();
        match outcome {
            Err(crate::SdkError::Remote { code, .. }) => assert_eq!(
                code,
                quanta_index_contract::TOP_K_OUT_OF_RANGE_CODE,
                "top_k={top_k}"
            ),
            other => panic!("top_k={top_k} must be refused with the shared code, got {other:?}"),
        }
        let sent = ok_or_fail!(query.requests.lock()).len();
        assert_eq!(sent, 0, "a refused top_k must not reach the transport");
    }
}

#[test]
fn text_query_builder_accepts_the_public_maximum_top_k() {
    let query = unused_query();
    let client = QuantaIndex::from_transports(query.clone(), unused_control(), unused_ingest());
    let _response = ok_or_fail!(
        client
            .lexical()
            .query()
            .native("needle")
            .active(RepoId::new("repo"), RevisionId::new("rev"))
            .top_k(quanta_index_contract::PUBLIC_TOP_K_MAX)
            .execute()
    );
    let captured = ok_or_fail!(only_query_request(query.as_ref()));
    let quanta_index_contract::SearchPlaneQueryIpcRequest::Text(request) = captured.payload else {
        panic!(
            "expected a text query on the wire, got {:?}",
            captured.payload
        );
    };
    assert_eq!(request.top_k, quanta_index_contract::PUBLIC_TOP_K_MAX);
}

// QI-BB-025: the hybrid-seed builder has its own request assembly path and
// must apply the same local gate as the text builders — it is the one route
// whose SDK builder does not go through `build_request`.
#[test]
fn hybrid_seed_builder_refuses_out_of_range_top_k_before_any_round_trip() {
    for top_k in [0, quanta_index_contract::PUBLIC_TOP_K_MAX + 1, u32::MAX] {
        let query = unused_query();
        let client = QuantaIndex::from_transports(query.clone(), unused_control(), unused_ingest());
        let outcome = client
            .search()
            .hybrid_seed()
            .native("needle")
            .semantic_text("needle")
            .active(repo_id(), revision_id())
            .top_k(top_k)
            .execute();
        match outcome {
            Err(crate::SdkError::Remote { code, .. }) => assert_eq!(
                code,
                quanta_index_contract::TOP_K_OUT_OF_RANGE_CODE,
                "top_k={top_k}"
            ),
            other => panic!("top_k={top_k} must be refused with the shared code, got {other:?}"),
        }
        let sent = ok_or_fail!(query.requests.lock()).len();
        assert_eq!(sent, 0, "a refused top_k must not reach the transport");
    }
}
