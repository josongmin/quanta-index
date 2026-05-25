use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use quanta_index_contract::lex::{
    LangId, SymbolKind, SymbolRecord, SymbolRelationship, SymbolSpan,
};
use quanta_index_contract::{
    BatchPublishReceipt, ChannelSeq, ChunkId, ChunkRecord, EmbeddingId, EmbeddingRecord,
    GenerationSelector, HybridQueryResponse, ManifestGeneration, PlannerStage, PlannerTraceEntry,
    RepoId, RepoMapMutationAck, RepoRelativePath, RevisionId, SearchExplanation,
    SearchPlaneActivationAck, SearchPlaneControlIpcRequestEnvelope,
    SearchPlaneControlIpcResponseEnvelope, SearchPlaneIngestIpcRequest,
    SearchPlaneIngestIpcRequestEnvelope, SearchPlaneIngestIpcResponse,
    SearchPlaneIngestIpcResponseEnvelope, SearchPlaneQueryIpcRequestEnvelope,
    SearchPlaneQueryIpcResponse, SearchPlaneQueryIpcResponseEnvelope,
    SearchPlaneSourcegraphQueryResponse, SemanticQueryResponse, SymbolId, TextQueryResponse,
};

use crate::{
    ConnectOptions, ControlTransport, IngestTransport, LexicalBatch, QuantaIndex, QueryTransport,
    SemanticBatch, Track,
};

macro_rules! ok_or_fail {
    ($expr:expr $(,)?) => {
        match $expr {
            Ok(value) => value,
            Err(err) => {
                assert!(false, "unexpected error: {err}");
                return;
            }
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
        wire_version: 1,
        name: "sample".into(),
        kind: SymbolKind::Function,
        span: SymbolSpan {
            path: "src/lib.rs".into(),
            byte_start: 0,
            byte_end: 10,
            line_start: 1,
            line_end: 1,
        },
        lang: LangId::Rust,
        parent: None,
        container_name: None,
        relationship: SymbolRelationship::Def,
    }
}

fn unused_query() -> Arc<StubQueryTransport> {
    Arc::new(StubQueryTransport::new(SearchPlaneQueryIpcResponse::Text(
        TextQueryResponse {
            generation: sample_generation_pin(),
            results: vec![],
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
        SearchPlaneIngestIpcResponse::LexicalReceipt(BatchPublishReceipt::default()),
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
fn semantic_query_builder_emits_active_selector_and_inline_vector_ref() {
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
        .vector(vec![0.1, 0.2, 0.3])
        .top_k(5)
        .execute();
    let _response = ok_or_fail!(response);
    let captured = ok_or_fail!(only_query_request(query.as_ref()));
    let quanta_index_contract::SearchPlaneQueryIpcRequest::Semantic(req) = &captured.payload else {
        assert!(
            false,
            "expected semantic request, got {payload:?}",
            payload = captured.payload
        );
        return;
    };
    assert_eq!(req.top_k, 5);
    assert!(req.query_vector_ref.is_some());
    assert!(matches!(
        req.generation_selector,
        Some(GenerationSelector::Active { .. })
    ));
}

#[test]
fn lexical_query_builder_carries_top_k_to_wire_contract() {
    let query = Arc::new(StubQueryTransport::new(SearchPlaneQueryIpcResponse::Text(
        TextQueryResponse {
            generation: sample_generation_pin(),
            results: vec![sample_hit()],
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
    let quanta_index_contract::SearchPlaneQueryIpcRequest::Text(req) = &captured.payload else {
        assert!(
            false,
            "expected text request, got {payload:?}",
            payload = captured.payload
        );
        return;
    };
    assert_eq!(
        req.top_k, 42,
        "QI-QRY-01: TextQueryRequest.top_k must be set from builder"
    );
}

#[test]
fn hybrid_search_builder_dispatches_hybrid_request_with_vector_handle() {
    let query = Arc::new(StubQueryTransport::new(
        SearchPlaneQueryIpcResponse::Hybrid(HybridQueryResponse {
            generation: sample_generation_pin(),
            results: vec![sample_hit()],
            explanation: sample_explanation(),
        }),
    ));
    let client = QuantaIndex::from_transports(query.clone(), unused_control(), unused_ingest());
    let _response = ok_or_fail!(
        client
            .search()
            .hybrid()
            .native("scope text")
            .vector_handle("handle-1")
            .active(repo_id(), revision_id())
            .top_k(7)
            .execute()
    );
    let captured = ok_or_fail!(only_query_request(query.as_ref()));
    let quanta_index_contract::SearchPlaneQueryIpcRequest::Hybrid(req) = &captured.payload else {
        assert!(
            false,
            "expected hybrid request, got {payload:?}",
            payload = captured.payload
        );
        return;
    };
    assert_eq!(req.top_k, 7);
    assert_eq!(req.text_query.top_k, 7);
    assert!(req.semantic_vector_ref.is_some());
}

#[test]
fn sourcegraph_query_builder_dispatches_dedicated_sourcegraph_request() {
    let query = Arc::new(StubQueryTransport::new(
        SearchPlaneQueryIpcResponse::Sourcegraph(SearchPlaneSourcegraphQueryResponse {
            generation: sample_generation_pin(),
            results: vec![sample_hit()],
        }),
    ));
    let client = QuantaIndex::from_transports(query.clone(), unused_control(), unused_ingest());
    let response = client
        .sourcegraph()
        .query()
        .source_syntax("repo:repo-1 lang:rust sample")
        .sg_version("sg-5.5.0")
        .pinned(sample_generation_pin())
        .top_k(9)
        .execute();
    let response = ok_or_fail!(response);
    assert_eq!(response.results.len(), 1);
    let captured = ok_or_fail!(only_query_request(query.as_ref()));
    let quanta_index_contract::SearchPlaneQueryIpcRequest::Sourcegraph(req) = &captured.payload
    else {
        assert!(
            false,
            "expected sourcegraph request, got {payload:?}",
            payload = captured.payload
        );
        return;
    };
    assert_eq!(req.source_syntax.as_ref(), "repo:repo-1 lang:rust sample");
    assert_eq!(req.sg_version.as_ref(), "sg-5.5.0");
    assert_eq!(req.generation, Some(sample_generation_pin()));
    assert_eq!(req.top_k, 9);
}

#[test]
fn lexical_publish_routes_through_ingest_transport_and_carries_typed_records() {
    let receipt = BatchPublishReceipt {
        first_seq: Some(ChannelSeq::new(0)),
        last_seq: Some(ChannelSeq::new(3)),
        sealed: true,
    };
    let ingest = Arc::new(StubIngestTransport::new(
        SearchPlaneIngestIpcResponse::LexicalReceipt(receipt.clone()),
    ));
    let client = QuantaIndex::from_transports(unused_query(), unused_control(), ingest.clone());
    let chunk = ChunkRecord {
        repo_relative_path: RepoRelativePath::new("src/lib.rs"),
        language: "rust".into(),
        start_line: 1,
        end_line: 4,
        snippet: "fn sample() {}".into(),
    };
    let batch =
        LexicalBatch::replace_generation(repo_id(), revision_id(), ManifestGeneration::new(1))
            .chunk_upsert(ChunkId::new("chunk-1"), chunk.clone())
            .symbol_upsert(SymbolId::new("sym-1"), sample_symbol());
    let observed = ok_or_fail!(client.lexical().publish(&batch));
    assert_eq!(observed, receipt);
    let captured = ok_or_fail!(only_ingest_request(ingest.as_ref()));
    let SearchPlaneIngestIpcRequest::PublishLexicalBatch(wire) = &captured.payload else {
        assert!(
            false,
            "expected PublishLexicalBatch, got {payload:?}",
            payload = captured.payload
        );
        return;
    };
    assert_eq!(wire.repo_id, repo_id());
    assert_eq!(wire.chunks.len(), 1);
    assert_eq!(wire.symbols.len(), 1);
    assert!(wire.seal);
    let Some(first_chunk) = wire.chunks.first() else {
        assert!(false, "expected one lexical chunk mutation");
        return;
    };
    let quanta_index_contract::LexicalChunkMutation::Upsert(upsert) = first_chunk else {
        assert!(false, "expected upsert mutation, got {first_chunk:?}");
        return;
    };
    assert_eq!(upsert.record, chunk);
}

#[test]
fn semantic_publish_routes_through_ingest_transport_and_carries_typed_embeddings() {
    let ingest = Arc::new(StubIngestTransport::new(
        SearchPlaneIngestIpcResponse::SemanticReceipt(BatchPublishReceipt::default()),
    ));
    let client = QuantaIndex::from_transports(unused_query(), unused_control(), ingest.clone());
    let batch =
        SemanticBatch::replace_generation(repo_id(), revision_id(), ManifestGeneration::new(1))
            .embedding_upsert(
                EmbeddingId::new("emb-1"),
                EmbeddingRecord {
                    owner_kind: "chunk".into(),
                    owner_id: "chunk-1".into(),
                    repo_relative_path: RepoRelativePath::new("src/lib.rs"),
                    language: LangId::Rust,
                    symbol_kind: None,
                    start_line: 1,
                    end_line: 4,
                    snippet: "fn sample() {}".into(),
                    vector: vec![0.1, 0.2],
                },
            );
    let _receipt = ok_or_fail!(client.semantic().publish(&batch));
    let captured = ok_or_fail!(only_ingest_request(ingest.as_ref()));
    let SearchPlaneIngestIpcRequest::PublishSemanticBatch(wire) = &captured.payload else {
        assert!(
            false,
            "expected PublishSemanticBatch, got {payload:?}",
            payload = captured.payload
        );
        return;
    };
    assert_eq!(wire.embeddings.len(), 1);
    assert!(wire.seal);
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
    let bundle = quanta_index_contract::RepoMapSourceBundle {
        repo_id: repo_id(),
        revision_id: revision_id(),
        manifest_generation: ManifestGeneration::new(1),
        snapshot_id: "snap".to_string(),
        projection_version: 1,
        authority_digest: "digest".to_string(),
        item_index_availability: "available".to_string(),
        graph_coverage_class: "full".to_string(),
        exactness_summary: "exact".to_string(),
        redaction_state: "Unredacted".to_string(),
        file_indices: vec![],
        call_edges: vec![],
        import_edges: vec![],
        chunk_records: vec![],
    };
    let observed = ok_or_fail!(client.repomap().publish(bundle));
    assert_eq!(observed.manifest_generation, ack.manifest_generation);
    let captured = ok_or_fail!(only_ingest_request(ingest.as_ref()));
    assert!(matches!(
        captured.payload,
        SearchPlaneIngestIpcRequest::PublishRepoMapBundle(_)
    ));
}

#[test]
fn lexical_publish_propagates_ingest_error_as_typed_remote() {
    let ingest = Arc::new(StubIngestTransport::new(
        SearchPlaneIngestIpcResponse::Error(quanta_index_contract::SearchPlaneIpcError {
            code: "INVALID_REQUEST".to_string(),
            message: "channel rejected".to_string(),
        }),
    ));
    let client = QuantaIndex::from_transports(unused_query(), unused_control(), ingest);
    let batch =
        LexicalBatch::replace_generation(repo_id(), revision_id(), ManifestGeneration::new(1));
    let err = client.lexical().publish(&batch).err();
    let Some(crate::SdkError::Remote { code, message }) = err else {
        assert!(false, "expected Remote error, got {err:?}");
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
    let captured = match control.requests.lock() {
        Ok(captured) => captured.first().cloned(),
        Err(err) => {
            assert!(false, "unexpected poisoned control requests: {err}");
            return;
        }
    };
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
        }),
    ));
    let client = QuantaIndex::from_transports(unused_query(), control, unused_ingest());
    let err = client
        .generations()
        .current(repo_id(), revision_id(), Track::Lexical)
        .err();
    let Some(crate::SdkError::Remote { code, .. }) = err else {
        assert!(false, "expected Remote error, got {err:?}");
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
