use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use quanta_index_channel::{
    BundleChannelSubscriber, open_lexical_subscriber, open_semantic_subscriber,
};
use quanta_index_contract::{
    ChunkId, ChunkRecord, EmbeddingId, EmbeddingRecord, GenerationSelector, HybridQueryResponse,
    ManifestGeneration, PlannerStage, PlannerTraceEntry, RepoId, RepoRelativePath, RevisionId,
    SearchExplanation, SearchPlaneActivationAck, SearchPlaneControlIpcRequestEnvelope,
    SearchPlaneControlIpcResponseEnvelope, SearchPlaneQueryIpcRequestEnvelope,
    SearchPlaneQueryIpcResponse, SearchPlaneQueryIpcResponseEnvelope, SearchPlaneTrackKind,
    SemanticQueryResponse, SymbolId, TextQueryResponse,
};
use quanta_index_contract::lex::{
    LangId, SymbolKind, SymbolRecord, SymbolRelationship, SymbolSpan, SymbolVisibility,
};

use crate::{ConnectOptions, ControlTransport, LexicalBatch, QuantaIndex, QueryTransport, SemanticBatch, Track};

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
            .ok_or_else(|| crate::SdkError::Protocol("missing stub control response".to_string()))?;
        Ok(SearchPlaneControlIpcResponseEnvelope {
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
        summary: "ok".to_string(),
        planner_trace: vec![PlannerTraceEntry {
            stage: PlannerStage::Plan,
            detail: "planned".to_string(),
        }],
        engines_touched: vec![quanta_index_contract::EngineTouched::Semantic],
        early_stop_reason: None,
    }
}

fn sample_symbol() -> SymbolRecord {
    SymbolRecord {
        wire_version: 1,
        name: "sample".into(),
        kind: SymbolKind::Function,
        span: SymbolSpan {
            file: quanta_index_contract::FileId::new("src/lib.rs"),
            start_byte: 0,
            end_byte: 10,
            start_line: 1,
            start_col_utf16: 0,
            end_line: 1,
            end_col_utf16: 10,
        },
        lang: LangId::new("rust"),
        parent: None,
        container_name: None,
        relationship: SymbolRelationship {
            is_definition: true,
            is_reference: false,
            visibility: SymbolVisibility::Public,
        },
    }
}

#[test]
fn connect_options_from_state_root_resolve_default_sockets() -> Result<(), Box<dyn std::error::Error>>
{
    let resolved = ConnectOptions::from_state_root("/tmp/qi-state").resolve()?;
    assert_eq!(resolved.state_root, Some(PathBuf::from("/tmp/qi-state")));
    assert_eq!(
        resolved.query_socket,
        PathBuf::from("/tmp/qi-state/search-plane/query.sock")
    );
    assert_eq!(
        resolved.control_socket,
        PathBuf::from("/tmp/qi-state/search-plane/control.sock")
    );
    Ok(())
}

#[test]
fn semantic_query_builder_emits_active_selector_and_inline_vector_ref(
) -> Result<(), Box<dyn std::error::Error>> {
    let query = Arc::new(StubQueryTransport::new(SearchPlaneQueryIpcResponse::Semantic(
        SemanticQueryResponse {
            generation: sample_generation_pin(),
            results: vec![sample_hit()],
            explanation: sample_explanation(),
        },
    )));
    let control = Arc::new(StubControlTransport::new(
        quanta_index_contract::SearchPlaneControlIpcResponse::ActivationAck(
            SearchPlaneActivationAck {
                repo_id: repo_id(),
                revision_id: revision_id(),
                manifest_generation: ManifestGeneration::new(7),
                manifest_digest: "digest".to_string(),
                tracks: vec![Track::Lexical],
            },
        ),
    ));
    let client = QuantaIndex::from_transports(None, query.clone(), control);
    let _response = client
        .semantic()
        .query()
        .vector(vec![0.1, 0.2, 0.3])
        .scope_native("lang:rust")
        .active(repo_id(), revision_id())
        .top_k(5)
        .execute()?;
    let requests = query
        .requests
        .lock()
        .map_err(|err| format!("query requests poisoned: {err}"))?;
    let payload = &requests[0].payload;
    match payload {
        quanta_index_contract::SearchPlaneQueryIpcRequest::Semantic(request) => {
            assert_eq!(request.query_text, "");
            assert!(request.query_vector.is_none());
            match request.query_vector_ref.as_ref() {
                Some(quanta_index_contract::SemanticVectorRef::Inline(vector)) => {
                    assert_eq!(vector, &vec![0.1, 0.2, 0.3]);
                }
                other => return Err(format!("unexpected vector ref: {other:?}").into()),
            }
            match request.generation_selector.as_ref() {
                Some(GenerationSelector::Active {
                    repo_id,
                    revision_id,
                }) => {
                    assert_eq!(repo_id.as_str(), "repo-1");
                    assert_eq!(revision_id.as_str(), "rev-1");
                }
                other => return Err(format!("unexpected generation selector: {other:?}").into()),
            }
            let scope = request.scope.as_ref().ok_or("missing scope")?;
            assert_eq!(scope.query_text, "lang:rust");
        }
        other => return Err(format!("unexpected query payload: {other:?}").into()),
    }
    Ok(())
}

#[test]
fn hybrid_query_builder_emits_text_and_vector_legs() -> Result<(), Box<dyn std::error::Error>> {
    let query = Arc::new(StubQueryTransport::new(SearchPlaneQueryIpcResponse::Hybrid(
        HybridQueryResponse {
            generation: sample_generation_pin(),
            results: vec![sample_hit()],
            explanation: sample_explanation(),
        },
    )));
    let control = Arc::new(StubControlTransport::new(
        quanta_index_contract::SearchPlaneControlIpcResponse::ActivationAck(
            SearchPlaneActivationAck {
                repo_id: repo_id(),
                revision_id: revision_id(),
                manifest_generation: ManifestGeneration::new(7),
                manifest_digest: "digest".to_string(),
                tracks: vec![Track::Lexical, Track::Semantic],
            },
        ),
    ));
    let client = QuantaIndex::from_transports(None, query.clone(), control);
    let _response = client
        .search()
        .hybrid()
        .native("trait Searcher")
        .vector(vec![1.0, 2.0])
        .active(repo_id(), revision_id())
        .top_k(10)
        .execute()?;
    let requests = query
        .requests
        .lock()
        .map_err(|err| format!("query requests poisoned: {err}"))?;
    match &requests[0].payload {
        quanta_index_contract::SearchPlaneQueryIpcRequest::Hybrid(request) => {
            assert_eq!(request.text_query.query_text, "trait Searcher");
            assert_eq!(request.semantic_query_text, "");
            assert_eq!(request.top_k, 10);
        }
        other => return Err(format!("unexpected query payload: {other:?}").into()),
    }
    Ok(())
}

#[test]
fn activation_builder_emits_track_set() -> Result<(), Box<dyn std::error::Error>> {
    let query = Arc::new(StubQueryTransport::new(SearchPlaneQueryIpcResponse::Text(
        TextQueryResponse {
            generation: sample_generation_pin(),
            results: vec![sample_hit()],
        },
    )));
    let control = Arc::new(StubControlTransport::new(
        quanta_index_contract::SearchPlaneControlIpcResponse::ActivationAck(
            SearchPlaneActivationAck {
                repo_id: repo_id(),
                revision_id: revision_id(),
                manifest_generation: ManifestGeneration::new(9),
                manifest_digest: "digest".to_string(),
                tracks: vec![Track::Lexical, Track::Semantic],
            },
        ),
    ));
    let client = QuantaIndex::from_transports(None, query, control.clone());
    let _ack = client
        .generations()
        .activate()
        .repo(repo_id())
        .revision(revision_id())
        .generation(ManifestGeneration::new(9))
        .manifest_digest("digest")
        .tracks([Track::Lexical, Track::Semantic, Track::Lexical])
        .commit()?;
    let requests = control
        .requests
        .lock()
        .map_err(|err| format!("control requests poisoned: {err}"))?;
    match &requests[0].payload {
        quanta_index_contract::SearchPlaneControlIpcRequest::ActivateGeneration(request) => {
            assert_eq!(request.tracks.len(), 2);
            assert_eq!(request.tracks[0], SearchPlaneTrackKind::Lexical);
            assert_eq!(request.tracks[1], SearchPlaneTrackKind::Semantic);
        }
        other => return Err(format!("unexpected control payload: {other:?}").into()),
    }
    Ok(())
}

#[test]
fn lexical_and_semantic_publish_encode_records_for_channel_consumers(
) -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let no_query = Arc::new(StubQueryTransport::new(SearchPlaneQueryIpcResponse::Text(
        TextQueryResponse {
            generation: sample_generation_pin(),
            results: vec![],
        },
    )));
    let no_control = Arc::new(StubControlTransport::new(
        quanta_index_contract::SearchPlaneControlIpcResponse::ActivationAck(
            SearchPlaneActivationAck {
                repo_id: repo_id(),
                revision_id: revision_id(),
                manifest_generation: ManifestGeneration::new(1),
                manifest_digest: "digest".to_string(),
                tracks: vec![Track::Lexical],
            },
        ),
    ));
    let client = QuantaIndex::from_transports(
        Some(dir.path().to_path_buf()),
        no_query,
        no_control,
    );
    let chunk = ChunkRecord {
        repo_relative_path: RepoRelativePath::new("src/lib.rs"),
        language: "rust".into(),
        start_line: 1,
        end_line: 4,
        snippet: "fn sample() {}".into(),
    };
    let lex_batch = LexicalBatch::replace_generation(
        repo_id(),
        revision_id(),
        ManifestGeneration::new(1),
    )
    .chunk_upsert(ChunkId::new("chunk-1"), chunk.clone())
    .symbol_upsert(SymbolId::new("sym-1"), sample_symbol());
    let sem_batch = SemanticBatch::replace_generation(
        repo_id(),
        revision_id(),
        ManifestGeneration::new(1),
    )
    .embedding_upsert(
        EmbeddingId::new("emb-1"),
        EmbeddingRecord {
            owner_kind: "chunk".into(),
            owner_id: "chunk-1".into(),
            repo_relative_path: RepoRelativePath::new("src/lib.rs"),
            language: LangId::new("rust"),
            symbol_kind: None,
            start_line: 1,
            end_line: 4,
            snippet: "fn sample() {}".into(),
            vector: vec![0.1, 0.2],
        },
    );
    let _lex_receipt = client.lexical().publish(&lex_batch)?;
    let _sem_receipt = client.semantic().publish(&sem_batch)?;
    let mut lex_sub = open_lexical_subscriber(dir.path())?;
    let mut sem_sub = open_semantic_subscriber(dir.path())?;
    let evt1 = lex_sub.next_event()?.ok_or("missing lexical evt1")?;
    let evt2 = lex_sub.next_event()?.ok_or("missing lexical evt2")?;
    let evt3 = lex_sub.next_event()?.ok_or("missing lexical evt3")?;
    let evt4 = lex_sub.next_event()?.ok_or("missing lexical evt4")?;
    match evt2.op {
        quanta_index_contract::LexicalChannelOp::UpsertChunk(op) => {
            let decoded: ChunkRecord = ciborium::de::from_reader(op.payload.as_slice())?;
            assert_eq!(decoded, chunk);
        }
        other => return Err(format!("unexpected lexical evt2: {other:?}").into()),
    }
    match evt3.op {
        quanta_index_contract::LexicalChannelOp::UpsertSymbol(op) => {
            let decoded: SymbolRecord = ciborium::de::from_reader(op.payload.as_slice())?;
            assert_eq!(decoded.name.as_str(), "sample");
        }
        other => return Err(format!("unexpected lexical evt3: {other:?}").into()),
    }
    assert!(matches!(
        evt4.op,
        quanta_index_contract::LexicalChannelOp::Seal(_)
    ));
    let sem_evt1 = sem_sub.next_event()?.ok_or("missing semantic evt1")?;
    let sem_evt2 = sem_sub.next_event()?.ok_or("missing semantic evt2")?;
    let sem_evt3 = sem_sub.next_event()?.ok_or("missing semantic evt3")?;
    match sem_evt2.op {
        quanta_index_contract::SemanticChannelOp::UpsertEmbedding(op) => {
            let decoded: EmbeddingRecord = ciborium::de::from_reader(op.payload.as_slice())?;
            assert_eq!(decoded.owner_id.as_ref(), "chunk-1");
        }
        other => return Err(format!("unexpected semantic evt2: {other:?}").into()),
    }
    assert!(matches!(
        sem_evt3.op,
        quanta_index_contract::SemanticChannelOp::Seal(_)
    ));
    lex_sub.ack(evt1.seq)?;
    lex_sub.ack(evt2.seq)?;
    lex_sub.ack(evt3.seq)?;
    lex_sub.ack(evt4.seq)?;
    sem_sub.ack(sem_evt1.seq)?;
    sem_sub.ack(sem_evt2.seq)?;
    sem_sub.ack(sem_evt3.seq)?;
    Ok(())
}
