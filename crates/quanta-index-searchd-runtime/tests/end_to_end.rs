//! End-to-end integration: publisher writes ops → searchd dispatcher consumes
//! and feeds adapters → searchd UDS query server returns matches.

#![forbid(unsafe_code)]
#![expect(
    clippy::disallowed_methods,
    reason = "test polling paths still use explicit Result fallback checks"
)]
#![expect(
    clippy::let_underscore_untyped,
    reason = "publisher ops intentionally discard ack payloads in integration setup"
)]
#![expect(
    clippy::wildcard_enum_match_arm,
    reason = "integration response checks intentionally collapse non-target variants"
)]

use std::error::Error;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use quanta_index_channel::{
    BundleChannelPublisher, open_lexical_publisher, open_semantic_publisher,
};
use quanta_index_contract::lex::{
    CommitRecord, CommitSha, LanguageCode, ParseNode, ParseTreeRecord,
    compute_parse_tree_source_hash,
};
use quanta_index_contract::{
    BridgeQueryRequest, BridgeScope, BridgeTarget, ChunkId, ChunkRecord, DeleteChunk, EmbeddingId,
    GenerationPin, HistoryQueryRequest, HybridQueryRequest, LexicalChannelOp, LexicalFullBundle,
    LqVisibility, ManifestGeneration, RepoId, RepoRelativePath, RevisionId,
    SearchPlaneQueryIpcRequest, SearchPlaneQueryIpcRequestEnvelope, SearchPlaneQueryIpcResponse,
    SearchPlaneQueryIpcResponseEnvelope, SemanticChannelOp, SemanticFullBundle,
    SemanticQueryRequest, SemanticVectorRef, StructuralQueryRequest, TextQueryRequest,
    TextQuerySyntax, UpsertChunk, UpsertCommit, UpsertEmbedding, UpsertParseTree,
};
use quanta_index_ipc::send_request;
use quanta_index_lq_bridge::TRANSLATOR_VERSION;
use quanta_index_searchd::app::SearchdConfig;
use quanta_index_searchd::app::searchd::drive;
use quanta_index_searchd_runtime::build_runtime;
use serde::ser::{Serialize, SerializeStruct, Serializer};

type TestResult = Result<(), Box<dyn Error>>;
static NEXT_SOCKET_ID: AtomicU64 = AtomicU64::new(0);
const READINESS_TIMEOUT: Duration = Duration::from_secs(15);

struct RepoMetadataPayload<'a> {
    fork: bool,
    archived: bool,
    visibility: LqVisibility,
    contexts: &'a [&'a str],
}

impl Serialize for RepoMetadataPayload<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("RepoMetadataPayload", 4)?;
        state.serialize_field("fork", &self.fork)?;
        state.serialize_field("archived", &self.archived)?;
        state.serialize_field("visibility", &self.visibility)?;
        state.serialize_field("contexts", &self.contexts)?;
        state.end()
    }
}

fn repo() -> RepoId {
    RepoId::new("repo-int")
}

fn revision() -> RevisionId {
    RevisionId::new("rev-int")
}

fn generation() -> ManifestGeneration {
    ManifestGeneration::new(7)
}

fn chunk_payload(text: &str) -> Result<Vec<u8>, Box<dyn Error>> {
    chunk_payload_with_metadata("", "", 0, 0, text)
}

fn chunk_payload_with_metadata(
    repo_relative_path: &str,
    language: &str,
    start_line: u32,
    end_line: u32,
    text: &str,
) -> Result<Vec<u8>, Box<dyn Error>> {
    let repo_relative_path = if repo_relative_path.is_empty() {
        "src/e2e.txt"
    } else {
        repo_relative_path
    };
    let language = if language.is_empty() {
        "text"
    } else {
        language
    };
    let record = ChunkRecord {
        chunk_id: ChunkId::new("payload-chunk"),
        repo_relative_path: RepoRelativePath::new(repo_relative_path),
        language: LanguageCode::new(language)
            .map_err(|err| -> Box<dyn Error> { format!("invalid language code: {err}").into() })?,
        start_byte: 0,
        end_byte: u32::try_from(text.len()).map_err(|err| -> Box<dyn Error> {
            format!("chunk text length overflow: {err}").into()
        })?,
        start_line,
        end_line,
        snippet: text.to_string().into_boxed_str(),
        indexed_text: text.to_string().into_boxed_str(),
        text_digest: "text:e2e".to_string().into_boxed_str(),
        shape_digest: "shape:e2e".to_string().into_boxed_str(),
        structural: None,
        parent_chunk_id: None,
    };
    let mut buf = Vec::new();
    ciborium::into_writer(&record, &mut buf)
        .map_err(|err| -> Box<dyn Error> { format!("encode chunk: {err}").into() })?;
    Ok(buf)
}

fn encode_cbor<T: Serialize>(value: &T, label: &str) -> Result<Vec<u8>, Box<dyn Error>> {
    let mut buf = Vec::new();
    ciborium::into_writer(value, &mut buf)
        .map_err(|err| -> Box<dyn Error> { format!("encode {label}: {err}").into() })?;
    Ok(buf)
}

fn history_commit_sha() -> CommitSha {
    CommitSha::from_bytes([
        0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd,
        0xef, 0x01, 0x23, 0x45, 0x67,
    ])
}

fn history_commit_record() -> CommitRecord {
    CommitRecord {
        wire_version: 1,
        sha: history_commit_sha(),
        parents: Vec::new(),
        author_time_ms: 11,
        committer_time_ms: 12,
        applied_at_ms: 13,
        author: "alice".to_string().into_boxed_str(),
        committer: "alice".to_string().into_boxed_str(),
        message: "fix: sample".to_string().into_boxed_str(),
        is_merge: false,
        tags: vec!["v1.0.0".to_string().into_boxed_str()],
    }
}

fn structural_tree_record() -> Result<ParseTreeRecord, Box<dyn Error>> {
    Ok(ParseTreeRecord {
        wire_version: 1,
        lang: LanguageCode::new("rust")
            .map_err(|err| -> Box<dyn Error> { format!("invalid tree lang: {err}").into() })?,
        root: ParseNode {
            kind: "function_item".to_string().into_boxed_str(),
            byte_start: 0,
            byte_end: 10,
            children: vec![
                ParseNode {
                    kind: "identifier".to_string().into_boxed_str(),
                    byte_start: 3,
                    byte_end: 7,
                    children: Vec::new(),
                },
                ParseNode {
                    kind: "block".to_string().into_boxed_str(),
                    byte_start: 8,
                    byte_end: 10,
                    children: Vec::new(),
                },
            ],
        },
        source_hash: compute_parse_tree_source_hash("fn main() {}"),
        role_tag_schema_version: 1,
        role_tags: Vec::new(),
    })
}

fn publish_structural_ready_fixture(state_root: &Path) -> TestResult {
    let publisher = open_lexical_publisher(state_root)?;
    let _ = publisher.publish(LexicalChannelOp::FullBundle(LexicalFullBundle {
        repo_id: repo(),
        revision_id: revision(),
        generation: generation(),
        payload: b"manifest".to_vec(),
    }))?;
    let _ = publisher.publish(LexicalChannelOp::UpsertChunk(UpsertChunk {
        repo_id: repo(),
        revision_id: revision(),
        generation: generation(),
        chunk_id: ChunkId::new("chunk-tree"),
        payload: chunk_payload_with_metadata("src/lib.rs", "rust", 1, 1, "fn main() {}")?,
    }))?;
    let tree = structural_tree_record()?;
    let mut payload = Vec::new();
    ciborium::into_writer(&tree, &mut payload)
        .map_err(|err| -> Box<dyn Error> { format!("encode parse tree: {err}").into() })?;
    let _ = publisher.publish(LexicalChannelOp::UpsertParseTree(UpsertParseTree {
        repo_id: repo(),
        revision_id: revision(),
        generation: generation(),
        chunk_id: ChunkId::new("chunk-tree"),
        payload,
    }))?;
    Ok(())
}

fn repo_metadata_payload(
    fork: bool,
    archived: bool,
    visibility: LqVisibility,
    contexts: &[&str],
) -> Result<Vec<u8>, Box<dyn Error>> {
    let record = RepoMetadataPayload {
        fork,
        archived,
        visibility,
        contexts,
    };
    let mut buf = Vec::new();
    ciborium::into_writer(&record, &mut buf)
        .map_err(|err| -> Box<dyn Error> { format!("encode repo metadata: {err}").into() })?;
    Ok(buf)
}

fn unique_socket_paths() -> (std::path::PathBuf, std::path::PathBuf) {
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let sequence = NEXT_SOCKET_ID.fetch_add(1, Ordering::Relaxed);
    let query = std::env::temp_dir().join(format!("qi-query-test-{pid}-{nanos}-{sequence}.sock"));
    let control =
        std::env::temp_dir().join(format!("qi-control-test-{pid}-{nanos}-{sequence}.sock"));
    (query, control)
}

fn build_config(state_root: &Path) -> SearchdConfig {
    let mut cfg = SearchdConfig::from_state_root(state_root.to_path_buf());
    // The unit socket path under tmpdir state root can exceed the 104-byte
    // AF_UNIX limit on macOS for long temp paths; use a flat path in
    // /tmp instead.
    let (query_socket, control_socket) = unique_socket_paths();
    cfg = SearchdConfig::with_socket_overrides(cfg, query_socket, control_socket);
    cfg
}

fn send_query_request(
    socket: &Path,
    request: &SearchPlaneQueryIpcRequestEnvelope,
) -> Result<SearchPlaneQueryIpcResponseEnvelope, quanta_index_ipc::IpcError> {
    send_request(socket, request)
}

fn wait_until<F>(timeout: Duration, mut cond: F) -> bool
where
    F: FnMut() -> bool,
{
    let start = Instant::now();
    while start.elapsed() < timeout {
        if cond() {
            return true;
        }
        thread::sleep(Duration::from_millis(10));
    }
    false
}

fn float_vec_to_bytes(vec: &[f32]) -> Result<Vec<u8>, Box<dyn Error>> {
    let owned: Vec<f32> = vec.to_vec();
    let mut out: Vec<u8> = Vec::new();
    ciborium::into_writer(&owned, &mut out)
        .map_err(|err| -> Box<dyn Error> { format!("ciborium encode embedding: {err}").into() })?;
    Ok(out)
}

fn float_vec_to_query_text(vec: &[f32]) -> String {
    vec.iter()
        .map(|v| format!("{v}"))
        .collect::<Vec<_>>()
        .join(" ")
}

#[test]
fn publish_dispatch_query_lexical_roundtrip() -> TestResult {
    let dir = tempfile::tempdir()?;
    let state_root = dir.path();

    // Publish 3 chunks + seal on the lexical track.
    {
        let publisher = open_lexical_publisher(state_root)?;
        let _ = publisher.publish(LexicalChannelOp::FullBundle(LexicalFullBundle {
            repo_id: repo(),
            revision_id: revision(),
            generation: generation(),
            payload: b"manifest".to_vec(),
        }))?;
        let _ = publisher.publish(LexicalChannelOp::UpsertChunk(UpsertChunk {
            repo_id: repo(),
            revision_id: revision(),
            generation: generation(),
            chunk_id: ChunkId::new("c1"),
            payload: chunk_payload("hello world")?,
        }))?;
        let _ = publisher.publish(LexicalChannelOp::UpsertChunk(UpsertChunk {
            repo_id: repo(),
            revision_id: revision(),
            generation: generation(),
            chunk_id: ChunkId::new("c2"),
            payload: chunk_payload("hello rust")?,
        }))?;
        let _ = publisher.publish(LexicalChannelOp::UpsertChunk(UpsertChunk {
            repo_id: repo(),
            revision_id: revision(),
            generation: generation(),
            chunk_id: ChunkId::new("c3"),
            payload: chunk_payload("goodbye")?,
        }))?;
        let _ = publisher.seal(repo(), revision(), generation())?;
    }

    let config = build_config(state_root);
    let runtime = build_runtime(config)?;
    let socket = runtime.query_server.socket_path().to_path_buf();
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_for_drive = Arc::clone(&shutdown);

    let join = thread::Builder::new()
        .name("searchd-test-driver".into())
        .spawn(move || drive(runtime, shutdown_for_drive))?;

    // Wait for socket to appear.
    if !wait_until(Duration::from_secs(2), || socket.exists()) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("socket never appeared".into());
    }
    let mut ready_candidates = None;
    if !wait_until(READINESS_TIMEOUT, || {
        let probe = lex_query("hello");
        match send_query_request(&socket, &probe) {
            Ok(resp) => match resp.payload {
                SearchPlaneQueryIpcResponse::Text(lex) => {
                    if lex.results.len() == 2 {
                        ready_candidates = Some(lex.results);
                        true
                    } else {
                        false
                    }
                }
                SearchPlaneQueryIpcResponse::Symbol(_)
                | SearchPlaneQueryIpcResponse::Semantic(_)
                | SearchPlaneQueryIpcResponse::Hybrid(_)
                | SearchPlaneQueryIpcResponse::History(_)
                | SearchPlaneQueryIpcResponse::Structural(_)
                | SearchPlaneQueryIpcResponse::Bridge(_)
                | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
                | SearchPlaneQueryIpcResponse::Explain(_)
                | quanta_index_contract::SearchPlaneQueryIpcResponse::RuntimeMetadata(_)
                | SearchPlaneQueryIpcResponse::Error(_) => false,
            },
            Err(_) => false,
        }
    }) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("dispatcher never sealed generation".into());
    }

    let candidates = ready_candidates
        .ok_or_else(|| "dispatcher readiness probe lost lexical results".to_string())?;
    if candidates.len() != 2 {
        return Err(format!("expected 2 candidates, got {}", candidates.len()).into());
    }
    let ids: Vec<String> = candidates.iter().map(|c| c.candidate_id.clone()).collect();
    if !ids.iter().any(|i| i == "c1") || !ids.iter().any(|i| i == "c2") {
        return Err(format!("missing expected ids: {ids:?}").into());
    }

    shutdown.store(true, Ordering::Release);
    match join.join() {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(e.into()),
        Err(panic) => Err(format!("driver panic: {panic:?}").into()),
    }
}

#[test]
fn publish_dispatch_query_sourcegraph_roundtrip() -> TestResult {
    let dir = tempfile::tempdir()?;
    let state_root = dir.path();

    {
        let publisher = open_lexical_publisher(state_root)?;
        let _ = publisher.publish(LexicalChannelOp::FullBundle(LexicalFullBundle {
            repo_id: repo(),
            revision_id: revision(),
            generation: generation(),
            payload: b"manifest".to_vec(),
        }))?;
        let _ = publisher.publish(LexicalChannelOp::UpsertChunk(UpsertChunk {
            repo_id: repo(),
            revision_id: revision(),
            generation: generation(),
            chunk_id: ChunkId::new("c1"),
            payload: chunk_payload("hello world")?,
        }))?;
        let _ = publisher.publish(LexicalChannelOp::UpsertChunk(UpsertChunk {
            repo_id: repo(),
            revision_id: revision(),
            generation: generation(),
            chunk_id: ChunkId::new("c2"),
            payload: chunk_payload("hello rust")?,
        }))?;
        let _ = publisher.publish(LexicalChannelOp::UpsertChunk(UpsertChunk {
            repo_id: repo(),
            revision_id: revision(),
            generation: generation(),
            chunk_id: ChunkId::new("c3"),
            payload: chunk_payload("goodbye")?,
        }))?;
        let _ = publisher.seal(repo(), revision(), generation())?;
    }

    let config = build_config(state_root);
    let runtime = build_runtime(config)?;
    let socket = runtime.query_server.socket_path().to_path_buf();
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_for_drive = Arc::clone(&shutdown);

    let join = thread::Builder::new()
        .name("searchd-sourcegraph-query-test".into())
        .spawn(move || drive(runtime, shutdown_for_drive))?;

    if !wait_until(Duration::from_secs(2), || socket.exists()) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("socket never appeared".into());
    }
    if !wait_until(READINESS_TIMEOUT, || {
        send_query_request(&socket, &lex_query("hello"))
            .map(|resp| matches!(resp.payload, SearchPlaneQueryIpcResponse::Text(_)))
            .unwrap_or(false)
    }) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("dispatcher never sealed sourcegraph generation".into());
    }

    let pin = GenerationPin::new(repo(), revision(), generation());
    let response = send_query_request(
        &socket,
        &SearchPlaneQueryIpcRequestEnvelope {
            request_id: 44,
            payload: SearchPlaneQueryIpcRequest::Text(TextQueryRequest {
                syntax: TextQuerySyntax::Sourcegraph,
                query_text: "hello".to_string(),
                generation: Some(pin.clone()),
                generation_selector: None,
                top_k: 50,
            }),
        },
    )?;
    let sourcegraph = match response.payload {
        SearchPlaneQueryIpcResponse::Text(payload) => payload,
        other => {
            shutdown.store(true, Ordering::Release);
            drop(join.join());
            return Err(format!("expected Text, got {other:?}").into());
        }
    };
    if sourcegraph.generation != pin {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("sourcegraph response generation did not echo request pin".into());
    }
    let ids: Vec<String> = sourcegraph
        .results
        .iter()
        .map(|candidate| candidate.candidate_id.clone())
        .collect();
    if !ids.iter().any(|id| id == "c1") || !ids.iter().any(|id| id == "c2") {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!("missing expected sourcegraph ids: {ids:?}").into());
    }

    shutdown.store(true, Ordering::Release);
    match join.join() {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(e.into()),
        Err(panic) => Err(format!("driver panic: {panic:?}").into()),
    }
}

#[test]
fn sourcegraph_path_and_lang_filters_execute_against_indexed_metadata() -> TestResult {
    let dir = tempfile::tempdir()?;
    let state_root = dir.path();

    {
        let publisher = open_lexical_publisher(state_root)?;
        let _ = publisher.publish(LexicalChannelOp::FullBundle(LexicalFullBundle {
            repo_id: repo(),
            revision_id: revision(),
            generation: generation(),
            payload: b"manifest".to_vec(),
        }))?;
        for (id, path, language, start_line, end_line, payload) in [
            ("alpha", "src/lib.rs", "rust", 3_u32, 8_u32, "needle alpha"),
            ("beta", "src/main.rs", "rust", 10_u32, 18_u32, "needle beta"),
            (
                "gamma",
                "src/lib.py",
                "python",
                20_u32,
                24_u32,
                "needle gamma",
            ),
        ] {
            let _ = publisher.publish(LexicalChannelOp::UpsertChunk(UpsertChunk {
                repo_id: repo(),
                revision_id: revision(),
                generation: generation(),
                chunk_id: ChunkId::new(id),
                payload: chunk_payload_with_metadata(
                    path, language, start_line, end_line, payload,
                )?,
            }))?;
        }
        let _ = publisher.seal(repo(), revision(), generation())?;
    }

    let config = build_config(state_root);
    let runtime = build_runtime(config)?;
    let socket = runtime.query_server.socket_path().to_path_buf();
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_for_drive = Arc::clone(&shutdown);
    let join = thread::Builder::new()
        .name("searchd-sourcegraph-metadata-test".into())
        .spawn(move || drive(runtime, shutdown_for_drive))?;

    if !wait_until(Duration::from_secs(2), || socket.exists()) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("socket never appeared".into());
    }

    let req = SearchPlaneQueryIpcRequestEnvelope {
        request_id: 7,
        payload: SearchPlaneQueryIpcRequest::Text(TextQueryRequest {
            syntax: TextQuerySyntax::Sourcegraph,
            query_text: "path:src/lib.rs lang:rust needle".to_string(),
            generation: Some(GenerationPin::new(repo(), revision(), generation())),
            generation_selector: None,
            top_k: 50,
        }),
    };

    if !wait_until(READINESS_TIMEOUT, || {
        send_query_request(&socket, &req)
            .map(|resp| !matches!(resp.payload, SearchPlaneQueryIpcResponse::Error(_)))
            .unwrap_or(false)
    }) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("sourcegraph metadata query never became ready".into());
    }

    let response = send_query_request(&socket, &req)?;
    let results = match response.payload {
        SearchPlaneQueryIpcResponse::Text(lexical) => lexical.results,
        other => {
            shutdown.store(true, Ordering::Release);
            drop(join.join());
            return Err(format!("expected Text, got {other:?}").into());
        }
    };
    if results.len() != 1 {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!("expected 1 metadata-filtered hit, got {results:?}").into());
    }
    let candidate = results
        .first()
        .ok_or_else(|| "metadata-filtered result missing first candidate".to_string())?;
    if candidate.candidate_id != "alpha" {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!("expected alpha, got {}", candidate.candidate_id).into());
    }
    if candidate.repo_relative_path.as_str() != "src/lib.rs" {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!(
            "expected src/lib.rs path, got {}",
            candidate.repo_relative_path.as_str()
        )
        .into());
    }
    if candidate.start_line != 3 || candidate.end_line != 8 {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!(
            "expected line span 3..8, got {}..{}",
            candidate.start_line, candidate.end_line
        )
        .into());
    }

    shutdown.store(true, Ordering::Release);
    match join.join() {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(e.into()),
        Err(panic) => Err(format!("driver panic: {panic:?}").into()),
    }
}

#[test]
fn history_query_returns_typed_generation_not_ready_error() -> TestResult {
    let dir = tempfile::tempdir()?;
    let state_root = dir.path();
    let config = build_config(state_root);
    let runtime = build_runtime(config)?;
    let socket = runtime.query_server.socket_path().to_path_buf();
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_for_drive = Arc::clone(&shutdown);
    let join = thread::Builder::new()
        .name("searchd-history-generation-not-ready-test".into())
        .spawn(move || drive(runtime, shutdown_for_drive))?;

    if !wait_until(Duration::from_secs(2), || socket.exists()) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("socket never appeared".into());
    }

    let err = wait_for_typed_error(
        &socket,
        &history_query("type:commit fix"),
        READINESS_TIMEOUT,
    )?;
    if err.code != "HISTORY_GENERATION_NOT_READY" {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!("expected HISTORY_GENERATION_NOT_READY, got {}", err.code).into());
    }
    if !err.message.contains("not yet materialized") {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!("unexpected generation-not-ready message: {}", err.message).into());
    }

    shutdown.store(true, Ordering::Release);
    match join.join() {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(e.into()),
        Err(panic) => Err(format!("driver panic: {panic:?}").into()),
    }
}

#[test]
fn history_query_returns_typed_producer_unavailable_without_lexical_fallback() -> TestResult {
    let dir = tempfile::tempdir()?;
    let state_root = dir.path();

    {
        let publisher = open_lexical_publisher(state_root)?;
        let _ = publisher.publish(LexicalChannelOp::FullBundle(LexicalFullBundle {
            repo_id: repo(),
            revision_id: revision(),
            generation: generation(),
            payload: b"manifest".to_vec(),
        }))?;
        let _ = publisher.publish(LexicalChannelOp::UpsertChunk(UpsertChunk {
            repo_id: repo(),
            revision_id: revision(),
            generation: generation(),
            chunk_id: ChunkId::new("history-fallback"),
            payload: chunk_payload("fix only lives in lexical content")?,
        }))?;
        let _ = publisher.seal(repo(), revision(), generation())?;
    }

    let config = build_config(state_root);
    let runtime = build_runtime(config)?;
    let socket = runtime.query_server.socket_path().to_path_buf();
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_for_drive = Arc::clone(&shutdown);
    let join = thread::Builder::new()
        .name("searchd-history-producer-unavailable-test".into())
        .spawn(move || drive(runtime, shutdown_for_drive))?;

    if !wait_until(Duration::from_secs(2), || socket.exists()) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("socket never appeared".into());
    }
    if !wait_until(READINESS_TIMEOUT, || {
        send_query_request(&socket, &lex_query("fix"))
            .map(|resp| matches!(resp.payload, SearchPlaneQueryIpcResponse::Text(_)))
            .unwrap_or(false)
    }) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("lexical fixture never became queryable".into());
    }

    let err = wait_for_typed_error(&socket, &history_query("fix"), READINESS_TIMEOUT)?;
    if err.code != "HISTORY_PRODUCER_UNAVAILABLE" {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!("expected HISTORY_PRODUCER_UNAVAILABLE, got {}", err.code).into());
    }
    if !err.message.contains("producer data is unavailable") {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!("unexpected producer-unavailable message: {}", err.message).into());
    }

    shutdown.store(true, Ordering::Release);
    match join.join() {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(e.into()),
        Err(panic) => Err(format!("driver panic: {panic:?}").into()),
    }
}

#[test]
fn history_query_returns_typed_shard_unavailable_when_diff_shard_missing() -> TestResult {
    let dir = tempfile::tempdir()?;
    let state_root = dir.path();

    {
        let publisher = open_lexical_publisher(state_root)?;
        let _ = publisher.publish(LexicalChannelOp::FullBundle(LexicalFullBundle {
            repo_id: repo(),
            revision_id: revision(),
            generation: generation(),
            payload: b"manifest".to_vec(),
        }))?;
        let _ = publisher.publish(LexicalChannelOp::UpsertChunk(UpsertChunk {
            repo_id: repo(),
            revision_id: revision(),
            generation: generation(),
            chunk_id: ChunkId::new("history-lex"),
            payload: chunk_payload("history shard lexical proof")?,
        }))?;
        let _ = publisher.publish(LexicalChannelOp::UpsertCommit(UpsertCommit {
            repo_id: repo(),
            revision_id: revision(),
            generation: generation(),
            payload: encode_cbor(&history_commit_record(), "history commit")?,
        }))?;
        let _ = publisher.seal(repo(), revision(), generation())?;
    }

    let config = build_config(state_root);
    let runtime = build_runtime(config)?;
    let socket = runtime.query_server.socket_path().to_path_buf();
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_for_drive = Arc::clone(&shutdown);
    let join = thread::Builder::new()
        .name("searchd-history-shard-unavailable-test".into())
        .spawn(move || drive(runtime, shutdown_for_drive))?;

    if !wait_until(Duration::from_secs(2), || socket.exists()) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("socket never appeared".into());
    }
    if !wait_until(READINESS_TIMEOUT, || {
        send_query_request(&socket, &lex_query("history"))
            .map(|resp| matches!(resp.payload, SearchPlaneQueryIpcResponse::Text(_)))
            .unwrap_or(false)
    }) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("history lexical proof never became ready".into());
    }

    let err = wait_for_typed_error(
        &socket,
        &history_query("type:diff history"),
        READINESS_TIMEOUT,
    )?;
    if err.code != "HISTORY_SHARD_UNAVAILABLE" {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!("expected HISTORY_SHARD_UNAVAILABLE, got {}", err.code).into());
    }
    if !err.message.contains("diff shard is unavailable") {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!("unexpected shard-unavailable message: {}", err.message).into());
    }

    shutdown.store(true, Ordering::Release);
    match join.join() {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(e.into()),
        Err(panic) => Err(format!("driver panic: {panic:?}").into()),
    }
}

#[test]
fn hybrid_query_requires_joint_seal() -> TestResult {
    let dir = tempfile::tempdir()?;
    let state_root = dir.path();

    // Publish a sealed lexical generation but NO semantic seal yet.
    {
        let publisher = open_lexical_publisher(state_root)?;
        let _ = publisher.publish(LexicalChannelOp::UpsertChunk(UpsertChunk {
            repo_id: repo(),
            revision_id: revision(),
            generation: generation(),
            chunk_id: ChunkId::new("c1"),
            payload: chunk_payload("only lex sealed")?,
        }))?;
        let _ = publisher.seal(repo(), revision(), generation())?;
    }

    let config = build_config(state_root);
    let runtime = build_runtime(config)?;
    let socket = runtime.query_server.socket_path().to_path_buf();
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_for_drive = Arc::clone(&shutdown);
    let join = thread::Builder::new()
        .name("searchd-test-driver".into())
        .spawn(move || drive(runtime, shutdown_for_drive))?;

    if !wait_until(Duration::from_secs(2), || socket.exists()) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("socket never appeared".into());
    }
    // Wait until lex seal is consumed.
    if !wait_until(READINESS_TIMEOUT, || {
        let probe = lex_query("only");
        send_query_request(&socket, &probe)
            .map(|r| !matches!(r.payload, SearchPlaneQueryIpcResponse::Error(_)))
            .unwrap_or(false)
    }) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("lex seal not consumed".into());
    }

    // Hybrid query should fail NOT_READY because semantic side is unsealed.
    let hybrid_req = SearchPlaneQueryIpcRequestEnvelope {
        request_id: 1,
        payload: SearchPlaneQueryIpcRequest::Hybrid(HybridQueryRequest {
            text_query: TextQueryRequest {
                syntax: TextQuerySyntax::Sourcegraph,
                query_text: "only".to_string(),
                generation: Some(GenerationPin::new(repo(), revision(), generation())),
                generation_selector: None,
                top_k: 50,
            },
            semantic_query_text: Some("1.0 0.0".to_string()),
            generation: Some(GenerationPin::new(repo(), revision(), generation())),
            semantic_vector: None,
            semantic_vector_ref: None,
            generation_selector: None,
            top_k: 5,
        }),
    };
    let response = send_query_request(&socket, &hybrid_req)?;
    let err = match response.payload {
        SearchPlaneQueryIpcResponse::Error(err) => err,
        other => {
            shutdown.store(true, Ordering::Release);
            drop(join.join());
            return Err(format!("expected Error, got {other:?}").into());
        }
    };
    if err.code != "NOT_READY" {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!("expected NOT_READY, got {}", err.code).into());
    }

    shutdown.store(true, Ordering::Release);
    match join.join() {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(e.into()),
        Err(panic) => Err(format!("driver panic: {panic:?}").into()),
    }
}

#[test]
fn hybrid_query_succeeds_when_both_tracks_sealed() -> TestResult {
    let dir = tempfile::tempdir()?;
    let state_root = dir.path();

    let lex_vec_a = [1.0_f32, 0.0_f32];
    let lex_vec_b = [0.0_f32, 1.0_f32];

    {
        let lex_pub = open_lexical_publisher(state_root)?;
        let _ = lex_pub.publish(LexicalChannelOp::UpsertChunk(UpsertChunk {
            repo_id: repo(),
            revision_id: revision(),
            generation: generation(),
            chunk_id: ChunkId::new("alpha"),
            payload: chunk_payload("sphinx of quartz")?,
        }))?;
        let _ = lex_pub.publish(LexicalChannelOp::UpsertChunk(UpsertChunk {
            repo_id: repo(),
            revision_id: revision(),
            generation: generation(),
            chunk_id: ChunkId::new("beta"),
            payload: chunk_payload("sphinx riddles")?,
        }))?;
        let _ = lex_pub.seal(repo(), revision(), generation())?;
    }
    {
        let sem_pub = open_semantic_publisher(state_root)?;
        let _ = sem_pub.publish(SemanticChannelOp::FullBundle(SemanticFullBundle {
            repo_id: repo(),
            revision_id: revision(),
            generation: generation(),
            payload: Vec::new(),
        }))?;
        let _ = sem_pub.publish(SemanticChannelOp::UpsertEmbedding(UpsertEmbedding {
            repo_id: repo(),
            revision_id: revision(),
            generation: generation(),
            embedding_id: EmbeddingId::new("alpha"),
            payload: float_vec_to_bytes(&lex_vec_a)?,
        }))?;
        let _ = sem_pub.publish(SemanticChannelOp::UpsertEmbedding(UpsertEmbedding {
            repo_id: repo(),
            revision_id: revision(),
            generation: generation(),
            embedding_id: EmbeddingId::new("beta"),
            payload: float_vec_to_bytes(&lex_vec_b)?,
        }))?;
        let _ = sem_pub.seal(repo(), revision(), generation())?;
    }

    let config = build_config(state_root);
    let runtime = build_runtime(config)?;
    let socket = runtime.query_server.socket_path().to_path_buf();
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_for_drive = Arc::clone(&shutdown);
    let join = thread::Builder::new()
        .name("searchd-test-driver".into())
        .spawn(move || drive(runtime, shutdown_for_drive))?;

    if !wait_until(Duration::from_secs(2), || socket.exists()) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("socket never appeared".into());
    }
    let pin = GenerationPin::new(repo(), revision(), generation());
    // Wait for joint seal: hybrid must stop returning Error.
    if !wait_until(READINESS_TIMEOUT, || {
        let req = SearchPlaneQueryIpcRequestEnvelope {
            request_id: 0,
            payload: SearchPlaneQueryIpcRequest::Hybrid(HybridQueryRequest {
                text_query: TextQueryRequest {
                    syntax: TextQuerySyntax::Sourcegraph,
                    query_text: "sphinx".to_string(),
                    generation: Some(pin.clone()),
                    generation_selector: None,
                    top_k: 50,
                },
                semantic_query_text: Some(float_vec_to_query_text(&[1.0_f32, 0.0_f32])),
                generation: Some(pin.clone()),
                semantic_vector: None,
                semantic_vector_ref: None,
                generation_selector: None,
                top_k: 5,
            }),
        };
        send_query_request(&socket, &req)
            .map(|r| !matches!(r.payload, SearchPlaneQueryIpcResponse::Error(_)))
            .unwrap_or(false)
    }) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("joint seal never reached".into());
    }

    // Final hybrid: must return both candidates fused with alpha ranked first
    // (alpha matches both lexical substring AND nearest semantic vector to
    // [1,0]).
    let req = SearchPlaneQueryIpcRequestEnvelope {
        request_id: 99,
        payload: SearchPlaneQueryIpcRequest::Hybrid(HybridQueryRequest {
            text_query: TextQueryRequest {
                syntax: TextQuerySyntax::Sourcegraph,
                query_text: "sphinx".to_string(),
                generation: Some(pin.clone()),
                generation_selector: None,
                top_k: 50,
            },
            semantic_query_text: Some(float_vec_to_query_text(&[1.0_f32, 0.0_f32])),
            generation: Some(pin),
            semantic_vector: None,
            semantic_vector_ref: None,
            generation_selector: None,
            top_k: 5,
        }),
    };
    let response = send_query_request(&socket, &req)?;
    let candidates = match response.payload {
        SearchPlaneQueryIpcResponse::Hybrid(h) => h.results,
        other => {
            shutdown.store(true, Ordering::Release);
            drop(join.join());
            return Err(format!("expected Hybrid, got {other:?}").into());
        }
    };
    if candidates.is_empty() {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("hybrid returned no candidates".into());
    }
    let top_id = candidates
        .first()
        .map(|c| c.candidate_id.clone())
        .unwrap_or_default();
    if top_id != "alpha" {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!("expected alpha top, got {top_id}").into());
    }

    shutdown.store(true, Ordering::Release);
    match join.join() {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(e.into()),
        Err(panic) => Err(format!("driver panic: {panic:?}").into()),
    }
}

#[test]
fn hybrid_query_with_explicit_semantic_vector_ignores_query_text() -> TestResult {
    let dir = tempfile::tempdir()?;
    let state_root = dir.path();

    {
        let lex_pub = open_lexical_publisher(state_root)?;
        let _ = lex_pub.publish(LexicalChannelOp::UpsertChunk(UpsertChunk {
            repo_id: repo(),
            revision_id: revision(),
            generation: generation(),
            chunk_id: ChunkId::new("alpha"),
            payload: chunk_payload("sphinx of quartz")?,
        }))?;
        let _ = lex_pub.publish(LexicalChannelOp::UpsertChunk(UpsertChunk {
            repo_id: repo(),
            revision_id: revision(),
            generation: generation(),
            chunk_id: ChunkId::new("beta"),
            payload: chunk_payload("sphinx riddles")?,
        }))?;
        let _ = lex_pub.seal(repo(), revision(), generation())?;
    }
    {
        let sem_pub = open_semantic_publisher(state_root)?;
        let _ = sem_pub.publish(SemanticChannelOp::FullBundle(SemanticFullBundle {
            repo_id: repo(),
            revision_id: revision(),
            generation: generation(),
            payload: Vec::new(),
        }))?;
        let _ = sem_pub.publish(SemanticChannelOp::UpsertEmbedding(UpsertEmbedding {
            repo_id: repo(),
            revision_id: revision(),
            generation: generation(),
            embedding_id: EmbeddingId::new("alpha"),
            payload: float_vec_to_bytes(&[1.0_f32, 0.0_f32])?,
        }))?;
        let _ = sem_pub.publish(SemanticChannelOp::UpsertEmbedding(UpsertEmbedding {
            repo_id: repo(),
            revision_id: revision(),
            generation: generation(),
            embedding_id: EmbeddingId::new("beta"),
            payload: float_vec_to_bytes(&[0.0_f32, 1.0_f32])?,
        }))?;
        let _ = sem_pub.seal(repo(), revision(), generation())?;
    }

    let config = build_config(state_root);
    let runtime = build_runtime(config)?;
    let socket = runtime.query_server.socket_path().to_path_buf();
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_for_drive = Arc::clone(&shutdown);
    let join = thread::Builder::new()
        .name("searchd-hybrid-explicit-vector-test".into())
        .spawn(move || drive(runtime, shutdown_for_drive))?;

    if !wait_until(Duration::from_secs(2), || socket.exists()) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("socket never appeared".into());
    }

    let pin = GenerationPin::new(repo(), revision(), generation());
    let req = SearchPlaneQueryIpcRequestEnvelope {
        request_id: 98,
        payload: SearchPlaneQueryIpcRequest::Hybrid(HybridQueryRequest {
            text_query: TextQueryRequest {
                syntax: TextQuerySyntax::Sourcegraph,
                query_text: "sphinx".to_string(),
                generation: Some(pin.clone()),
                generation_selector: None,
                top_k: 50,
            },
            semantic_query_text: Some("not a float vector".to_string()),
            semantic_vector: None,
            semantic_vector_ref: Some(SemanticVectorRef::Inline(vec![1.0_f32, 0.0_f32])),
            generation: Some(pin),
            generation_selector: None,
            top_k: 5,
        }),
    };

    if !wait_until(READINESS_TIMEOUT, || {
        send_query_request(&socket, &req)
            .map(|resp| !matches!(resp.payload, SearchPlaneQueryIpcResponse::Error(_)))
            .unwrap_or(false)
    }) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("hybrid explicit-vector query never became ready".into());
    }

    let response = send_query_request(&socket, &req)?;
    let results = match response.payload {
        SearchPlaneQueryIpcResponse::Hybrid(hybrid) => hybrid.results,
        other => {
            shutdown.store(true, Ordering::Release);
            drop(join.join());
            return Err(format!("expected Hybrid, got {other:?}").into());
        }
    };
    let top_id = results
        .first()
        .map(|candidate| candidate.candidate_id.clone())
        .unwrap_or_default();
    if top_id != "alpha" {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!("expected alpha top, got {top_id}").into());
    }

    shutdown.store(true, Ordering::Release);
    match join.join() {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(e.into()),
        Err(panic) => Err(format!("driver panic: {panic:?}").into()),
    }
}

#[test]
fn hybrid_query_rejects_generation_pin_mismatch() -> TestResult {
    let dir = tempfile::tempdir()?;
    let state_root = dir.path();
    let config = build_config(state_root);
    let runtime = build_runtime(config)?;
    let socket = runtime.query_server.socket_path().to_path_buf();
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_for_drive = Arc::clone(&shutdown);
    let join = thread::Builder::new()
        .name("searchd-hybrid-pin-mismatch-test".into())
        .spawn(move || drive(runtime, shutdown_for_drive))?;

    if !wait_until(Duration::from_secs(2), || socket.exists()) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("socket never appeared".into());
    }

    let response = send_query_request(
        &socket,
        &SearchPlaneQueryIpcRequestEnvelope {
            request_id: 40,
            payload: SearchPlaneQueryIpcRequest::Hybrid(HybridQueryRequest {
                text_query: TextQueryRequest {
                    syntax: TextQuerySyntax::Sourcegraph,
                    query_text: "needle".to_string(),
                    generation: Some(GenerationPin::new(
                        repo(),
                        revision(),
                        ManifestGeneration::new(8),
                    )),
                    generation_selector: None,
                    top_k: 50,
                },
                semantic_query_text: Some("1.0 0.0".to_string()),
                generation: Some(GenerationPin::new(repo(), revision(), generation())),
                semantic_vector: None,
                semantic_vector_ref: None,
                generation_selector: None,
                top_k: 1,
            }),
        },
    )?;
    let err = match response.payload {
        SearchPlaneQueryIpcResponse::Error(err) => err,
        other => {
            shutdown.store(true, Ordering::Release);
            drop(join.join());
            return Err(format!("expected Error, got {other:?}").into());
        }
    };
    if err.code != "INVALID_REQUEST" {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!("expected INVALID_REQUEST, got {}", err.code).into());
    }
    if !err
        .message
        .contains("hybrid: lexical generation does not match semantic generation")
    {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!("unexpected mismatch message: {}", err.message).into());
    }

    shutdown.store(true, Ordering::Release);
    match join.join() {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(e.into()),
        Err(panic) => Err(format!("driver panic: {panic:?}").into()),
    }
}

#[test]
fn sourcegraph_context_filter_executes_against_repo_metadata_surface() -> TestResult {
    let dir = tempfile::tempdir()?;
    let state_root = dir.path();

    {
        let lex_pub = open_lexical_publisher(state_root)?;
        let _ = lex_pub.publish(LexicalChannelOp::FullBundle(LexicalFullBundle {
            repo_id: repo(),
            revision_id: revision(),
            generation: generation(),
            payload: repo_metadata_payload(
                false,
                false,
                LqVisibility::Public,
                &["global", "team-search"],
            )?,
        }))?;
        let _ = lex_pub.publish(LexicalChannelOp::UpsertChunk(UpsertChunk {
            repo_id: repo(),
            revision_id: revision(),
            generation: generation(),
            chunk_id: ChunkId::new("alpha"),
            payload: chunk_payload("needle")?,
        }))?;
        let _ = lex_pub.seal(repo(), revision(), generation())?;
    }

    let config = build_config(state_root);
    let runtime = build_runtime(config)?;
    let socket = runtime.query_server.socket_path().to_path_buf();
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_for_drive = Arc::clone(&shutdown);
    let join = thread::Builder::new()
        .name("searchd-sourcegraph-context-test".into())
        .spawn(move || drive(runtime, shutdown_for_drive))?;

    if !wait_until(Duration::from_secs(2), || socket.exists()) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("socket never appeared".into());
    }

    let req = SearchPlaneQueryIpcRequestEnvelope {
        request_id: 17,
        payload: SearchPlaneQueryIpcRequest::Text(TextQueryRequest {
            syntax: TextQuerySyntax::Sourcegraph,
            query_text: "fork:no archived:no visibility:public context:global needle".to_string(),
            generation: Some(GenerationPin::new(repo(), revision(), generation())),
            generation_selector: None,
            top_k: 50,
        }),
    };

    if !wait_until(READINESS_TIMEOUT, || {
        send_query_request(&socket, &req)
            .map(|resp| match resp.payload {
                SearchPlaneQueryIpcResponse::Error(err) => err.code != "NOT_READY",
                _ => true,
            })
            .unwrap_or(false)
    }) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("sourcegraph context filter never progressed past NOT_READY".into());
    }

    let response = send_query_request(&socket, &req)?;
    let results = match response.payload {
        SearchPlaneQueryIpcResponse::Text(text) => text.results,
        other => {
            shutdown.store(true, Ordering::Release);
            drop(join.join());
            return Err(format!("expected Text, got {other:?}").into());
        }
    };
    if results.len() != 1 {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!("expected one context-filtered hit, got {results:?}").into());
    }
    if results
        .first()
        .map(|candidate| candidate.candidate_id.as_str())
        != Some("alpha")
    {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!("expected alpha hit, got {results:?}").into());
    }

    shutdown.store(true, Ordering::Release);
    match join.join() {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(e.into()),
        Err(panic) => Err(format!("driver panic: {panic:?}").into()),
    }
}

#[test]
fn hybrid_query_visibility_filter_executes_against_repo_metadata_surface() -> TestResult {
    let dir = tempfile::tempdir()?;
    let state_root = dir.path();

    {
        let lex_pub = open_lexical_publisher(state_root)?;
        let _ = lex_pub.publish(LexicalChannelOp::FullBundle(LexicalFullBundle {
            repo_id: repo(),
            revision_id: revision(),
            generation: generation(),
            payload: repo_metadata_payload(false, false, LqVisibility::Public, &["global"])?,
        }))?;
        let _ = lex_pub.publish(LexicalChannelOp::UpsertChunk(UpsertChunk {
            repo_id: repo(),
            revision_id: revision(),
            generation: generation(),
            chunk_id: ChunkId::new("alpha"),
            payload: chunk_payload("needle")?,
        }))?;
        let _ = lex_pub.seal(repo(), revision(), generation())?;
    }
    {
        let sem_pub = open_semantic_publisher(state_root)?;
        let _ = sem_pub.publish(SemanticChannelOp::FullBundle(SemanticFullBundle {
            repo_id: repo(),
            revision_id: revision(),
            generation: generation(),
            payload: Vec::new(),
        }))?;
        let _ = sem_pub.publish(SemanticChannelOp::UpsertEmbedding(UpsertEmbedding {
            repo_id: repo(),
            revision_id: revision(),
            generation: generation(),
            embedding_id: EmbeddingId::new("alpha"),
            payload: float_vec_to_bytes(&[1.0_f32, 0.0_f32])?,
        }))?;
        let _ = sem_pub.seal(repo(), revision(), generation())?;
    }

    let config = build_config(state_root);
    let runtime = build_runtime(config)?;
    let socket = runtime.query_server.socket_path().to_path_buf();
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_for_drive = Arc::clone(&shutdown);
    let join = thread::Builder::new()
        .name("searchd-hybrid-lowering-error-test".into())
        .spawn(move || drive(runtime, shutdown_for_drive))?;

    if !wait_until(Duration::from_secs(2), || socket.exists()) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("socket never appeared".into());
    }

    let pin = GenerationPin::new(repo(), revision(), generation());
    let req = SearchPlaneQueryIpcRequestEnvelope {
        request_id: 41,
        payload: SearchPlaneQueryIpcRequest::Hybrid(HybridQueryRequest {
            text_query: TextQueryRequest {
                syntax: TextQuerySyntax::Sourcegraph,
                query_text: "fork:no archived:no visibility:public context:global needle"
                    .to_string(),
                generation: Some(pin.clone()),
                generation_selector: None,
                top_k: 50,
            },
            semantic_query_text: Some(float_vec_to_query_text(&[1.0_f32, 0.0_f32])),
            generation: Some(pin),
            semantic_vector: None,
            semantic_vector_ref: None,
            generation_selector: None,
            top_k: 1,
        }),
    };

    if !wait_until(READINESS_TIMEOUT, || {
        send_query_request(&socket, &req)
            .map(|resp| match resp.payload {
                SearchPlaneQueryIpcResponse::Error(err) => err.code != "NOT_READY",
                _ => true,
            })
            .unwrap_or(false)
    }) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("hybrid visibility filter never progressed past NOT_READY".into());
    }

    let response = send_query_request(&socket, &req)?;
    let results = match response.payload {
        SearchPlaneQueryIpcResponse::Hybrid(hybrid) => hybrid.results,
        other => {
            shutdown.store(true, Ordering::Release);
            drop(join.join());
            return Err(format!("expected Hybrid, got {other:?}").into());
        }
    };
    if results.len() != 1 {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!("expected one hybrid hit, got {results:?}").into());
    }
    if results
        .first()
        .map(|candidate| candidate.candidate_id.as_str())
        != Some("alpha")
    {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!("expected alpha top hit, got {results:?}").into());
    }

    shutdown.store(true, Ordering::Release);
    match join.join() {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(e.into()),
        Err(panic) => Err(format!("driver panic: {panic:?}").into()),
    }
}

#[test]
fn semantic_only_query_requires_semantic_seal() -> TestResult {
    let dir = tempfile::tempdir()?;
    let state_root = dir.path();
    let config = build_config(state_root);
    let runtime = build_runtime(config)?;
    let socket = runtime.query_server.socket_path().to_path_buf();
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_for_drive = Arc::clone(&shutdown);
    let join = thread::Builder::new()
        .name("searchd-test-driver".into())
        .spawn(move || drive(runtime, shutdown_for_drive))?;

    if !wait_until(Duration::from_secs(2), || socket.exists()) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("socket never appeared".into());
    }

    let req = SearchPlaneQueryIpcRequestEnvelope {
        request_id: 0,
        payload: SearchPlaneQueryIpcRequest::Semantic(SemanticQueryRequest {
            query_text: Some("1.0".to_string()),
            query_vector: None,
            query_vector_ref: None,
            generation: Some(GenerationPin::new(repo(), revision(), generation())),
            generation_selector: None,
            lexical_scope: None,
            top_k: 3,
        }),
    };
    let response = send_query_request(&socket, &req)?;
    let err = match response.payload {
        SearchPlaneQueryIpcResponse::Error(e) => e,
        other => {
            shutdown.store(true, Ordering::Release);
            drop(join.join());
            return Err(format!("expected Error, got {other:?}").into());
        }
    };
    if err.code != "NOT_READY" {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!("expected NOT_READY, got {}", err.code).into());
    }

    shutdown.store(true, Ordering::Release);
    match join.join() {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(e.into()),
        Err(panic) => Err(format!("driver panic: {panic:?}").into()),
    }
}

#[test]
fn semantic_query_without_lexical_scope_returns_global_nearest_hit() -> TestResult {
    let dir = tempfile::tempdir()?;
    let state_root = dir.path();

    {
        let sem_pub = open_semantic_publisher(state_root)?;
        let _ = sem_pub.publish(SemanticChannelOp::FullBundle(SemanticFullBundle {
            repo_id: repo(),
            revision_id: revision(),
            generation: generation(),
            payload: Vec::new(),
        }))?;
        let _ = sem_pub.publish(SemanticChannelOp::UpsertEmbedding(UpsertEmbedding {
            repo_id: repo(),
            revision_id: revision(),
            generation: generation(),
            embedding_id: EmbeddingId::new("alpha"),
            payload: float_vec_to_bytes(&[1.0_f32, 0.0_f32])?,
        }))?;
        let _ = sem_pub.publish(SemanticChannelOp::UpsertEmbedding(UpsertEmbedding {
            repo_id: repo(),
            revision_id: revision(),
            generation: generation(),
            embedding_id: EmbeddingId::new("beta"),
            payload: float_vec_to_bytes(&[0.0_f32, 1.0_f32])?,
        }))?;
        let _ = sem_pub.seal(repo(), revision(), generation())?;
    }

    let config = build_config(state_root);
    let runtime = build_runtime(config)?;
    let socket = runtime.query_server.socket_path().to_path_buf();
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_for_drive = Arc::clone(&shutdown);
    let join = thread::Builder::new()
        .name("searchd-semantic-no-scope-success-test".into())
        .spawn(move || drive(runtime, shutdown_for_drive))?;

    if !wait_until(Duration::from_secs(2), || socket.exists()) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("socket never appeared".into());
    }

    let pin = GenerationPin::new(repo(), revision(), generation());
    let req = SearchPlaneQueryIpcRequestEnvelope {
        request_id: 42,
        payload: SearchPlaneQueryIpcRequest::Semantic(SemanticQueryRequest {
            query_text: Some(float_vec_to_query_text(&[1.0_f32, 0.0_f32])),
            query_vector: None,
            query_vector_ref: None,
            generation: Some(pin.clone()),
            generation_selector: None,
            lexical_scope: None,
            top_k: 1,
        }),
    };

    if !wait_until(READINESS_TIMEOUT, || {
        send_query_request(&socket, &req)
            .map(|resp| !matches!(resp.payload, SearchPlaneQueryIpcResponse::Error(_)))
            .unwrap_or(false)
    }) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("semantic no-scope query never became ready".into());
    }

    let response = send_query_request(&socket, &req)?;
    let results = match response.payload {
        SearchPlaneQueryIpcResponse::Semantic(semantic) => {
            if semantic.generation != pin {
                shutdown.store(true, Ordering::Release);
                drop(join.join());
                return Err("semantic response generation did not echo request pin".into());
            }
            semantic.results
        }
        other => {
            shutdown.store(true, Ordering::Release);
            drop(join.join());
            return Err(format!("expected Semantic, got {other:?}").into());
        }
    };
    let ids: Vec<String> = results
        .iter()
        .map(|candidate| candidate.candidate_id.clone())
        .collect();
    if ids != ["alpha".to_string()] {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!("expected global nearest [alpha], got {ids:?}").into());
    }

    shutdown.store(true, Ordering::Release);
    match join.join() {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(e.into()),
        Err(panic) => Err(format!("driver panic: {panic:?}").into()),
    }
}

#[test]
fn semantic_query_with_explicit_query_vector_ignores_query_text() -> TestResult {
    let dir = tempfile::tempdir()?;
    let state_root = dir.path();

    {
        let sem_pub = open_semantic_publisher(state_root)?;
        let _ = sem_pub.publish(SemanticChannelOp::FullBundle(SemanticFullBundle {
            repo_id: repo(),
            revision_id: revision(),
            generation: generation(),
            payload: Vec::new(),
        }))?;
        let _ = sem_pub.publish(SemanticChannelOp::UpsertEmbedding(UpsertEmbedding {
            repo_id: repo(),
            revision_id: revision(),
            generation: generation(),
            embedding_id: EmbeddingId::new("alpha"),
            payload: float_vec_to_bytes(&[1.0_f32, 0.0_f32])?,
        }))?;
        let _ = sem_pub.publish(SemanticChannelOp::UpsertEmbedding(UpsertEmbedding {
            repo_id: repo(),
            revision_id: revision(),
            generation: generation(),
            embedding_id: EmbeddingId::new("beta"),
            payload: float_vec_to_bytes(&[0.0_f32, 1.0_f32])?,
        }))?;
        let _ = sem_pub.seal(repo(), revision(), generation())?;
    }

    let config = build_config(state_root);
    let runtime = build_runtime(config)?;
    let socket = runtime.query_server.socket_path().to_path_buf();
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_for_drive = Arc::clone(&shutdown);
    let join = thread::Builder::new()
        .name("searchd-semantic-explicit-vector-test".into())
        .spawn(move || drive(runtime, shutdown_for_drive))?;

    if !wait_until(Duration::from_secs(2), || socket.exists()) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("socket never appeared".into());
    }

    let pin = GenerationPin::new(repo(), revision(), generation());
    let req = SearchPlaneQueryIpcRequestEnvelope {
        request_id: 97,
        payload: SearchPlaneQueryIpcRequest::Semantic(SemanticQueryRequest {
            query_text: Some("not numeric".to_string()),
            query_vector: None,
            query_vector_ref: Some(SemanticVectorRef::Inline(vec![1.0_f32, 0.0_f32])),
            generation: Some(pin),
            generation_selector: None,
            lexical_scope: None,
            top_k: 1,
        }),
    };

    if !wait_until(READINESS_TIMEOUT, || {
        send_query_request(&socket, &req)
            .map(|resp| !matches!(resp.payload, SearchPlaneQueryIpcResponse::Error(_)))
            .unwrap_or(false)
    }) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("semantic explicit-vector query never became ready".into());
    }

    let response = send_query_request(&socket, &req)?;
    let results = match response.payload {
        SearchPlaneQueryIpcResponse::Semantic(semantic) => semantic.results,
        other => {
            shutdown.store(true, Ordering::Release);
            drop(join.join());
            return Err(format!("expected Semantic, got {other:?}").into());
        }
    };
    let top_id = results
        .first()
        .map(|candidate| candidate.candidate_id.clone())
        .unwrap_or_default();
    if top_id != "alpha" {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!("expected alpha top, got {top_id}").into());
    }

    shutdown.store(true, Ordering::Release);
    match join.join() {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(e.into()),
        Err(panic) => Err(format!("driver panic: {panic:?}").into()),
    }
}

#[test]
fn semantic_query_with_handle_ref_resolves_active_generation_vector() -> TestResult {
    let dir = tempfile::tempdir()?;
    let state_root = dir.path();

    {
        let sem_pub = open_semantic_publisher(state_root)?;
        let _ = sem_pub.publish(SemanticChannelOp::FullBundle(SemanticFullBundle {
            repo_id: repo(),
            revision_id: revision(),
            generation: generation(),
            payload: Vec::new(),
        }))?;
        let _ = sem_pub.publish(SemanticChannelOp::UpsertEmbedding(UpsertEmbedding {
            repo_id: repo(),
            revision_id: revision(),
            generation: generation(),
            embedding_id: EmbeddingId::new("alpha"),
            payload: float_vec_to_bytes(&[1.0_f32, 0.0_f32])?,
        }))?;
        let _ = sem_pub.publish(SemanticChannelOp::UpsertEmbedding(UpsertEmbedding {
            repo_id: repo(),
            revision_id: revision(),
            generation: generation(),
            embedding_id: EmbeddingId::new("beta"),
            payload: float_vec_to_bytes(&[0.0_f32, 1.0_f32])?,
        }))?;
        let _ = sem_pub.seal(repo(), revision(), generation())?;
    }

    let config = build_config(state_root);
    let runtime = build_runtime(config)?;
    let socket = runtime.query_server.socket_path().to_path_buf();
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_for_drive = Arc::clone(&shutdown);
    let join = thread::Builder::new()
        .name("searchd-semantic-handle-ref-test".into())
        .spawn(move || drive(runtime, shutdown_for_drive))?;

    if !wait_until(Duration::from_secs(2), || socket.exists()) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("socket never appeared".into());
    }

    let pin = GenerationPin::new(repo(), revision(), generation());
    let req = SearchPlaneQueryIpcRequestEnvelope {
        request_id: 96,
        payload: SearchPlaneQueryIpcRequest::Semantic(SemanticQueryRequest {
            query_text: Some("still not numeric".to_string()),
            query_vector: None,
            query_vector_ref: Some(SemanticVectorRef::Handle("alpha".into())),
            generation: Some(pin),
            generation_selector: None,
            lexical_scope: None,
            top_k: 1,
        }),
    };

    if !wait_until(READINESS_TIMEOUT, || {
        send_query_request(&socket, &req)
            .map(|resp| !matches!(resp.payload, SearchPlaneQueryIpcResponse::Error(_)))
            .unwrap_or(false)
    }) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("semantic handle-ref query never became ready".into());
    }

    let response = send_query_request(&socket, &req)?;
    let results = match response.payload {
        SearchPlaneQueryIpcResponse::Semantic(semantic) => semantic.results,
        other => {
            shutdown.store(true, Ordering::Release);
            drop(join.join());
            return Err(format!("expected Semantic, got {other:?}").into());
        }
    };
    let top_id = results
        .first()
        .map(|candidate| candidate.candidate_id.clone())
        .unwrap_or_default();
    if top_id != "alpha" {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!("expected alpha top via handle ref, got {top_id}").into());
    }

    shutdown.store(true, Ordering::Release);
    match join.join() {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(e.into()),
        Err(panic) => Err(format!("driver panic: {panic:?}").into()),
    }
}

#[test]
fn semantic_query_rejects_generation_pin_mismatch_with_lexical_scope() -> TestResult {
    let dir = tempfile::tempdir()?;
    let state_root = dir.path();
    let config = build_config(state_root);
    let runtime = build_runtime(config)?;
    let socket = runtime.query_server.socket_path().to_path_buf();
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_for_drive = Arc::clone(&shutdown);
    let join = thread::Builder::new()
        .name("searchd-semantic-pin-mismatch-test".into())
        .spawn(move || drive(runtime, shutdown_for_drive))?;

    if !wait_until(Duration::from_secs(2), || socket.exists()) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("socket never appeared".into());
    }

    let response = send_query_request(
        &socket,
        &SearchPlaneQueryIpcRequestEnvelope {
            request_id: 43,
            payload: SearchPlaneQueryIpcRequest::Semantic(SemanticQueryRequest {
                query_text: Some(float_vec_to_query_text(&[1.0_f32, 0.0_f32])),
                query_vector: None,
                query_vector_ref: None,
                generation: Some(GenerationPin::new(repo(), revision(), generation())),
                generation_selector: None,
                lexical_scope: Some(TextQueryRequest {
                    syntax: TextQuerySyntax::Sourcegraph,
                    query_text: "scope".to_string(),
                    generation: Some(GenerationPin::new(
                        repo(),
                        revision(),
                        ManifestGeneration::new(8),
                    )),
                    generation_selector: None,
                    top_k: 1,
                }),
                top_k: 1,
            }),
        },
    )?;
    let err = match response.payload {
        SearchPlaneQueryIpcResponse::Error(err) => err,
        other => {
            shutdown.store(true, Ordering::Release);
            drop(join.join());
            return Err(format!("expected Error, got {other:?}").into());
        }
    };
    if err.code != "INVALID_REQUEST" {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!("expected INVALID_REQUEST, got {}", err.code).into());
    }
    if !err
        .message
        .contains("semantic: scope generation does not match semantic request generation")
    {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!("unexpected mismatch message: {}", err.message).into());
    }

    shutdown.store(true, Ordering::Release);
    match join.join() {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(e.into()),
        Err(panic) => Err(format!("driver panic: {panic:?}").into()),
    }
}

#[test]
fn semantic_query_surfaces_scoped_lexical_lowering_typed_error() -> TestResult {
    let dir = tempfile::tempdir()?;
    let state_root = dir.path();

    {
        let sem_pub = open_semantic_publisher(state_root)?;
        let _ = sem_pub.publish(SemanticChannelOp::FullBundle(SemanticFullBundle {
            repo_id: repo(),
            revision_id: revision(),
            generation: generation(),
            payload: Vec::new(),
        }))?;
        let _ = sem_pub.publish(SemanticChannelOp::UpsertEmbedding(UpsertEmbedding {
            repo_id: repo(),
            revision_id: revision(),
            generation: generation(),
            embedding_id: EmbeddingId::new("alpha"),
            payload: float_vec_to_bytes(&[1.0_f32, 0.0_f32])?,
        }))?;
        let _ = sem_pub.seal(repo(), revision(), generation())?;
    }

    let config = build_config(state_root);
    let runtime = build_runtime(config)?;
    let socket = runtime.query_server.socket_path().to_path_buf();
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_for_drive = Arc::clone(&shutdown);
    let join = thread::Builder::new()
        .name("searchd-semantic-lowering-error-test".into())
        .spawn(move || drive(runtime, shutdown_for_drive))?;

    if !wait_until(Duration::from_secs(2), || socket.exists()) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("socket never appeared".into());
    }

    let pin = GenerationPin::new(repo(), revision(), generation());
    let req = SearchPlaneQueryIpcRequestEnvelope {
        request_id: 44,
        payload: SearchPlaneQueryIpcRequest::Semantic(SemanticQueryRequest {
            query_text: Some(float_vec_to_query_text(&[1.0_f32, 0.0_f32])),
            query_vector: None,
            query_vector_ref: None,
            generation: Some(pin.clone()),
            generation_selector: None,
            lexical_scope: Some(TextQueryRequest {
                syntax: TextQuerySyntax::Sourcegraph,
                query_text: "index:no scoped".to_string(),
                generation: Some(pin),
                generation_selector: None,
                top_k: 1,
            }),
            top_k: 1,
        }),
    };

    if !wait_until(READINESS_TIMEOUT, || {
        send_query_request(&socket, &req)
            .map(|resp| match resp.payload {
                SearchPlaneQueryIpcResponse::Error(err) => err.code != "NOT_READY",
                _ => true,
            })
            .unwrap_or(false)
    }) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("semantic lowering error never progressed past NOT_READY".into());
    }

    let response = send_query_request(&socket, &req)?;
    let err = match response.payload {
        SearchPlaneQueryIpcResponse::Error(err) => err,
        other => {
            shutdown.store(true, Ordering::Release);
            drop(join.join());
            return Err(format!("expected Error, got {other:?}").into());
        }
    };
    if err.code != "BRIDGE_UNSUPPORTED_DIRECTIVE" {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!("expected BRIDGE_UNSUPPORTED_DIRECTIVE, got {}", err.code).into());
    }
    if !err.message.contains("index:no") {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!("unexpected lowering error message: {}", err.message).into());
    }

    shutdown.store(true, Ordering::Release);
    match join.join() {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(e.into()),
        Err(panic) => Err(format!("driver panic: {panic:?}").into()),
    }
}

#[test]
fn semantic_query_with_lexical_scope_returns_intersection_only() -> TestResult {
    let dir = tempfile::tempdir()?;
    let state_root = dir.path();
    let pin = GenerationPin::new(repo(), revision(), generation());

    {
        let lex_pub = open_lexical_publisher(state_root)?;
        let _ = lex_pub.publish(LexicalChannelOp::UpsertChunk(UpsertChunk {
            repo_id: repo(),
            revision_id: revision(),
            generation: generation(),
            chunk_id: ChunkId::new("alpha"),
            payload: chunk_payload("scope needle")?,
        }))?;
        let _ = lex_pub.publish(LexicalChannelOp::UpsertChunk(UpsertChunk {
            repo_id: repo(),
            revision_id: revision(),
            generation: generation(),
            chunk_id: ChunkId::new("beta"),
            payload: chunk_payload("scope miss")?,
        }))?;
        let _ = lex_pub.publish(LexicalChannelOp::UpsertChunk(UpsertChunk {
            repo_id: repo(),
            revision_id: revision(),
            generation: generation(),
            chunk_id: ChunkId::new("gamma"),
            payload: chunk_payload("outside needle")?,
        }))?;
        let _ = lex_pub.seal(repo(), revision(), generation())?;
    }
    {
        let sem_pub = open_semantic_publisher(state_root)?;
        let _ = sem_pub.publish(SemanticChannelOp::FullBundle(SemanticFullBundle {
            repo_id: repo(),
            revision_id: revision(),
            generation: generation(),
            payload: Vec::new(),
        }))?;
        let _ = sem_pub.publish(SemanticChannelOp::UpsertEmbedding(UpsertEmbedding {
            repo_id: repo(),
            revision_id: revision(),
            generation: generation(),
            embedding_id: EmbeddingId::new("alpha"),
            payload: float_vec_to_bytes(&[1.0_f32, 0.0_f32])?,
        }))?;
        let _ = sem_pub.publish(SemanticChannelOp::UpsertEmbedding(UpsertEmbedding {
            repo_id: repo(),
            revision_id: revision(),
            generation: generation(),
            embedding_id: EmbeddingId::new("beta"),
            payload: float_vec_to_bytes(&[0.0_f32, 1.0_f32])?,
        }))?;
        let _ = sem_pub.publish(SemanticChannelOp::UpsertEmbedding(UpsertEmbedding {
            repo_id: repo(),
            revision_id: revision(),
            generation: generation(),
            embedding_id: EmbeddingId::new("gamma"),
            payload: float_vec_to_bytes(&[1.0_f32, 0.0_f32])?,
        }))?;
        let _ = sem_pub.seal(repo(), revision(), generation())?;
    }

    let config = build_config(state_root);
    let runtime = build_runtime(config)?;
    let socket = runtime.query_server.socket_path().to_path_buf();
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_for_drive = Arc::clone(&shutdown);
    let join = thread::Builder::new()
        .name("searchd-semantic-scope-test".into())
        .spawn(move || drive(runtime, shutdown_for_drive))?;

    if !wait_until(Duration::from_secs(2), || socket.exists()) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("socket never appeared".into());
    }

    let req = SearchPlaneQueryIpcRequestEnvelope {
        request_id: 41,
        payload: SearchPlaneQueryIpcRequest::Semantic(SemanticQueryRequest {
            query_text: Some(float_vec_to_query_text(&[1.0_f32, 0.0_f32])),
            query_vector: None,
            query_vector_ref: None,
            generation: Some(pin.clone()),
            generation_selector: None,
            lexical_scope: Some(TextQueryRequest {
                syntax: TextQuerySyntax::Sourcegraph,
                query_text: "scope needle".to_string(),
                generation: Some(pin),
                generation_selector: None,
                top_k: 2,
            }),
            top_k: 2,
        }),
    };

    if !wait_until(READINESS_TIMEOUT, || {
        send_query_request(&socket, &req)
            .map(|resp| !matches!(resp.payload, SearchPlaneQueryIpcResponse::Error(_)))
            .unwrap_or(false)
    }) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("semantic scoped query never became ready".into());
    }

    let response = send_query_request(&socket, &req)?;
    let results = match response.payload {
        SearchPlaneQueryIpcResponse::Semantic(semantic) => semantic.results,
        other => {
            shutdown.store(true, Ordering::Release);
            drop(join.join());
            return Err(format!("expected Semantic, got {other:?}").into());
        }
    };
    let ids: Vec<String> = results
        .iter()
        .map(|candidate| candidate.candidate_id.clone())
        .collect();
    if ids != ["alpha".to_string()] {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!("expected scoped semantic intersection [alpha], got {ids:?}").into());
    }

    shutdown.store(true, Ordering::Release);
    match join.join() {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(e.into()),
        Err(panic) => Err(format!("driver panic: {panic:?}").into()),
    }
}

#[test]
fn semantic_scoped_query_ignores_out_of_scope_global_nearest_hit() -> TestResult {
    let dir = tempfile::tempdir()?;
    let state_root = dir.path();
    let pin = GenerationPin::new(repo(), revision(), generation());

    {
        let lex_pub = open_lexical_publisher(state_root)?;
        let _ = lex_pub.publish(LexicalChannelOp::UpsertChunk(UpsertChunk {
            repo_id: repo(),
            revision_id: revision(),
            generation: generation(),
            chunk_id: ChunkId::new("alpha"),
            payload: chunk_payload("outside")?,
        }))?;
        let _ = lex_pub.publish(LexicalChannelOp::UpsertChunk(UpsertChunk {
            repo_id: repo(),
            revision_id: revision(),
            generation: generation(),
            chunk_id: ChunkId::new("beta"),
            payload: chunk_payload("scope beta")?,
        }))?;
        let _ = lex_pub.publish(LexicalChannelOp::UpsertChunk(UpsertChunk {
            repo_id: repo(),
            revision_id: revision(),
            generation: generation(),
            chunk_id: ChunkId::new("gamma"),
            payload: chunk_payload("scope gamma")?,
        }))?;
        let _ = lex_pub.seal(repo(), revision(), generation())?;
    }
    {
        let sem_pub = open_semantic_publisher(state_root)?;
        let _ = sem_pub.publish(SemanticChannelOp::FullBundle(SemanticFullBundle {
            repo_id: repo(),
            revision_id: revision(),
            generation: generation(),
            payload: Vec::new(),
        }))?;
        let _ = sem_pub.publish(SemanticChannelOp::UpsertEmbedding(UpsertEmbedding {
            repo_id: repo(),
            revision_id: revision(),
            generation: generation(),
            embedding_id: EmbeddingId::new("alpha"),
            payload: float_vec_to_bytes(&[1.0_f32, 0.0_f32])?,
        }))?;
        let _ = sem_pub.publish(SemanticChannelOp::UpsertEmbedding(UpsertEmbedding {
            repo_id: repo(),
            revision_id: revision(),
            generation: generation(),
            embedding_id: EmbeddingId::new("beta"),
            payload: float_vec_to_bytes(&[0.9_f32, 0.1_f32])?,
        }))?;
        let _ = sem_pub.publish(SemanticChannelOp::UpsertEmbedding(UpsertEmbedding {
            repo_id: repo(),
            revision_id: revision(),
            generation: generation(),
            embedding_id: EmbeddingId::new("gamma"),
            payload: float_vec_to_bytes(&[0.0_f32, 1.0_f32])?,
        }))?;
        let _ = sem_pub.seal(repo(), revision(), generation())?;
    }

    let config = build_config(state_root);
    let runtime = build_runtime(config)?;
    let socket = runtime.query_server.socket_path().to_path_buf();
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_for_drive = Arc::clone(&shutdown);
    let join = thread::Builder::new()
        .name("searchd-semantic-scope-starvation-test".into())
        .spawn(move || drive(runtime, shutdown_for_drive))?;

    if !wait_until(Duration::from_secs(2), || socket.exists()) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("socket never appeared".into());
    }

    let req = SearchPlaneQueryIpcRequestEnvelope {
        request_id: 42,
        payload: SearchPlaneQueryIpcRequest::Semantic(SemanticQueryRequest {
            query_text: Some(float_vec_to_query_text(&[1.0_f32, 0.0_f32])),
            query_vector: None,
            query_vector_ref: None,
            generation: Some(pin.clone()),
            generation_selector: None,
            lexical_scope: Some(TextQueryRequest {
                syntax: TextQuerySyntax::Sourcegraph,
                query_text: "scope".to_string(),
                generation: Some(pin),
                generation_selector: None,
                top_k: 1,
            }),
            top_k: 1,
        }),
    };

    if !wait_until(READINESS_TIMEOUT, || {
        send_query_request(&socket, &req)
            .map(|resp| !matches!(resp.payload, SearchPlaneQueryIpcResponse::Error(_)))
            .unwrap_or(false)
    }) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("semantic scoped query never became ready".into());
    }

    let response = send_query_request(&socket, &req)?;
    let results = match response.payload {
        SearchPlaneQueryIpcResponse::Semantic(semantic) => semantic.results,
        other => {
            shutdown.store(true, Ordering::Release);
            drop(join.join());
            return Err(format!("expected Semantic, got {other:?}").into());
        }
    };
    let ids: Vec<String> = results
        .iter()
        .map(|candidate| candidate.candidate_id.clone())
        .collect();
    if ids != ["beta".to_string()] {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!("expected scoped semantic [beta], got {ids:?}").into());
    }

    shutdown.store(true, Ordering::Release);
    match join.join() {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(e.into()),
        Err(panic) => Err(format!("driver panic: {panic:?}").into()),
    }
}

#[test]
fn semantic_query_rejects_invalid_vector_with_typed_code() -> TestResult {
    let dir = tempfile::tempdir()?;
    let state_root = dir.path();

    {
        let sem_pub = open_semantic_publisher(state_root)?;
        let _ = sem_pub.publish(SemanticChannelOp::FullBundle(SemanticFullBundle {
            repo_id: repo(),
            revision_id: revision(),
            generation: generation(),
            payload: Vec::new(),
        }))?;
        let _ = sem_pub.seal(repo(), revision(), generation())?;
    }

    let config = build_config(state_root);
    let runtime = build_runtime(config)?;
    let socket = runtime.query_server.socket_path().to_path_buf();
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_for_drive = Arc::clone(&shutdown);
    let join = thread::Builder::new()
        .name("searchd-sem-invalid-vector-test".into())
        .spawn(move || drive(runtime, shutdown_for_drive))?;

    if !wait_until(Duration::from_secs(2), || socket.exists()) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("socket never appeared".into());
    }

    let req = SearchPlaneQueryIpcRequestEnvelope {
        request_id: 43,
        payload: SearchPlaneQueryIpcRequest::Semantic(SemanticQueryRequest {
            query_text: Some("NaN".to_string()),
            query_vector: None,
            query_vector_ref: None,
            generation: Some(GenerationPin::new(repo(), revision(), generation())),
            generation_selector: None,
            lexical_scope: None,
            top_k: 3,
        }),
    };

    if !wait_until(READINESS_TIMEOUT, || {
        send_query_request(&socket, &req)
            .map(|resp| match resp.payload {
                SearchPlaneQueryIpcResponse::Error(err) => err.code != "NOT_READY",
                _ => true,
            })
            .unwrap_or(false)
    }) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("semantic invalid-vector query never progressed past NOT_READY".into());
    }

    let response = send_query_request(&socket, &req)?;
    let err = match response.payload {
        SearchPlaneQueryIpcResponse::Error(err) => err,
        other => {
            shutdown.store(true, Ordering::Release);
            drop(join.join());
            return Err(format!("expected Error, got {other:?}").into());
        }
    };
    if err.code != "SEM_INVALID_VECTOR" {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!("expected SEM_INVALID_VECTOR, got {}", err.code).into());
    }

    shutdown.store(true, Ordering::Release);
    match join.join() {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(e.into()),
        Err(panic) => Err(format!("driver panic: {panic:?}").into()),
    }
}

#[test]
fn hybrid_query_rejects_zero_top_k_with_typed_code() -> TestResult {
    let dir = tempfile::tempdir()?;
    let state_root = dir.path();
    let config = build_config(state_root);
    let runtime = build_runtime(config)?;
    let socket = runtime.query_server.socket_path().to_path_buf();
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_for_drive = Arc::clone(&shutdown);
    let join = thread::Builder::new()
        .name("searchd-hybrid-top-k-test".into())
        .spawn(move || drive(runtime, shutdown_for_drive))?;

    if !wait_until(Duration::from_secs(2), || socket.exists()) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("socket never appeared".into());
    }

    let response = send_query_request(
        &socket,
        &SearchPlaneQueryIpcRequestEnvelope {
            request_id: 44,
            payload: SearchPlaneQueryIpcRequest::Hybrid(HybridQueryRequest {
                text_query: TextQueryRequest {
                    syntax: TextQuerySyntax::Sourcegraph,
                    query_text: "needle".to_string(),
                    generation: Some(GenerationPin::new(repo(), revision(), generation())),
                    generation_selector: None,
                    top_k: 50,
                },
                semantic_query_text: Some("1.0 0.0".to_string()),
                generation: Some(GenerationPin::new(repo(), revision(), generation())),
                semantic_vector: None,
                semantic_vector_ref: None,
                generation_selector: None,
                top_k: 0,
            }),
        },
    )?;
    let err = match response.payload {
        SearchPlaneQueryIpcResponse::Error(err) => err,
        other => {
            shutdown.store(true, Ordering::Release);
            drop(join.join());
            return Err(format!("expected Error, got {other:?}").into());
        }
    };
    if err.code != "HYB_TOP_K_INVALID" {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!("expected HYB_TOP_K_INVALID, got {}", err.code).into());
    }

    shutdown.store(true, Ordering::Release);
    match join.join() {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(e.into()),
        Err(panic) => Err(format!("driver panic: {panic:?}").into()),
    }
}

#[test]
fn hybrid_query_excludes_semantic_outsider_from_lexical_universe() -> TestResult {
    let dir = tempfile::tempdir()?;
    let state_root = dir.path();

    {
        let lex_pub = open_lexical_publisher(state_root)?;
        let _ = lex_pub.publish(LexicalChannelOp::UpsertChunk(UpsertChunk {
            repo_id: repo(),
            revision_id: revision(),
            generation: generation(),
            chunk_id: ChunkId::new("alpha"),
            payload: chunk_payload("outside")?,
        }))?;
        let _ = lex_pub.publish(LexicalChannelOp::UpsertChunk(UpsertChunk {
            repo_id: repo(),
            revision_id: revision(),
            generation: generation(),
            chunk_id: ChunkId::new("beta"),
            payload: chunk_payload("scope beta")?,
        }))?;
        let _ = lex_pub.publish(LexicalChannelOp::UpsertChunk(UpsertChunk {
            repo_id: repo(),
            revision_id: revision(),
            generation: generation(),
            chunk_id: ChunkId::new("gamma"),
            payload: chunk_payload("scope gamma")?,
        }))?;
        let _ = lex_pub.seal(repo(), revision(), generation())?;
    }
    {
        let sem_pub = open_semantic_publisher(state_root)?;
        let _ = sem_pub.publish(SemanticChannelOp::FullBundle(SemanticFullBundle {
            repo_id: repo(),
            revision_id: revision(),
            generation: generation(),
            payload: Vec::new(),
        }))?;
        let _ = sem_pub.publish(SemanticChannelOp::UpsertEmbedding(UpsertEmbedding {
            repo_id: repo(),
            revision_id: revision(),
            generation: generation(),
            embedding_id: EmbeddingId::new("alpha"),
            payload: float_vec_to_bytes(&[1.0_f32, 0.0_f32])?,
        }))?;
        let _ = sem_pub.publish(SemanticChannelOp::UpsertEmbedding(UpsertEmbedding {
            repo_id: repo(),
            revision_id: revision(),
            generation: generation(),
            embedding_id: EmbeddingId::new("beta"),
            payload: float_vec_to_bytes(&[0.9_f32, 0.1_f32])?,
        }))?;
        let _ = sem_pub.publish(SemanticChannelOp::UpsertEmbedding(UpsertEmbedding {
            repo_id: repo(),
            revision_id: revision(),
            generation: generation(),
            embedding_id: EmbeddingId::new("gamma"),
            payload: float_vec_to_bytes(&[0.0_f32, 1.0_f32])?,
        }))?;
        let _ = sem_pub.seal(repo(), revision(), generation())?;
    }

    let config = build_config(state_root);
    let runtime = build_runtime(config)?;
    let socket = runtime.query_server.socket_path().to_path_buf();
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_for_drive = Arc::clone(&shutdown);
    let join = thread::Builder::new()
        .name("searchd-hybrid-outsider-test".into())
        .spawn(move || drive(runtime, shutdown_for_drive))?;

    if !wait_until(Duration::from_secs(2), || socket.exists()) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("socket never appeared".into());
    }

    let pin = GenerationPin::new(repo(), revision(), generation());
    let req = SearchPlaneQueryIpcRequestEnvelope {
        request_id: 45,
        payload: SearchPlaneQueryIpcRequest::Hybrid(HybridQueryRequest {
            text_query: TextQueryRequest {
                syntax: TextQuerySyntax::Sourcegraph,
                query_text: "scope".to_string(),
                generation: Some(pin.clone()),
                generation_selector: None,
                top_k: 50,
            },
            semantic_query_text: Some(float_vec_to_query_text(&[1.0_f32, 0.0_f32])),
            generation: Some(pin),
            semantic_vector: None,
            semantic_vector_ref: None,
            generation_selector: None,
            top_k: 2,
        }),
    };

    if !wait_until(READINESS_TIMEOUT, || {
        send_query_request(&socket, &req)
            .map(|resp| !matches!(resp.payload, SearchPlaneQueryIpcResponse::Error(_)))
            .unwrap_or(false)
    }) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("hybrid query never became ready".into());
    }

    let response = send_query_request(&socket, &req)?;
    let results = match response.payload {
        SearchPlaneQueryIpcResponse::Hybrid(hybrid) => hybrid.results,
        other => {
            shutdown.store(true, Ordering::Release);
            drop(join.join());
            return Err(format!("expected Hybrid, got {other:?}").into());
        }
    };
    let ids: Vec<String> = results
        .iter()
        .map(|candidate| candidate.candidate_id.clone())
        .collect();
    if ids.first().map(String::as_str) != Some("beta") {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!("expected beta top, got {ids:?}").into());
    }
    if ids.iter().any(|id| id == "alpha") {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!("unexpected lexical outsider in hybrid results: {ids:?}").into());
    }

    shutdown.store(true, Ordering::Release);
    match join.join() {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(e.into()),
        Err(panic) => Err(format!("driver panic: {panic:?}").into()),
    }
}

#[test]
fn structural_query_returns_typed_generation_not_ready_error() -> TestResult {
    let dir = tempfile::tempdir()?;
    let state_root = dir.path();
    let config = build_config(state_root);
    let runtime = build_runtime(config)?;
    let socket = runtime.query_server.socket_path().to_path_buf();
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_for_drive = Arc::clone(&shutdown);
    let join = thread::Builder::new()
        .name("searchd-structural-test".into())
        .spawn(move || drive(runtime, shutdown_for_drive))?;

    if !wait_until(Duration::from_secs(2), || socket.exists()) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("socket never appeared".into());
    }

    let response = send_query_request(
        &socket,
        &SearchPlaneQueryIpcRequestEnvelope {
            request_id: 42,
            payload: SearchPlaneQueryIpcRequest::Structural(StructuralQueryRequest {
                text_query: TextQueryRequest {
                    syntax: TextQuerySyntax::Native,
                    query_text: "match { :[x] }".to_string(),
                    generation: Some(GenerationPin::new(repo(), revision(), generation())),
                    generation_selector: None,
                    top_k: 50,
                },
            }),
        },
    )?;
    let err = match response.payload {
        SearchPlaneQueryIpcResponse::Error(err) => err,
        other => {
            shutdown.store(true, Ordering::Release);
            drop(join.join());
            return Err(format!("expected Error, got {other:?}").into());
        }
    };
    if err.code != "STR_GENERATION_NOT_READY" {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!("expected STR_GENERATION_NOT_READY, got {}", err.code).into());
    }
    if !err.message.contains("not yet materialized") {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!(
            "expected generation-not-ready structural message, got {}",
            err.message
        )
        .into());
    }

    shutdown.store(true, Ordering::Release);
    match join.join() {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(e.into()),
        Err(panic) => Err(format!("driver panic: {panic:?}").into()),
    }
}

#[test]
fn structural_query_returns_typed_shard_unavailable_error() -> TestResult {
    let dir = tempfile::tempdir()?;
    let state_root = dir.path();

    {
        let publisher = open_lexical_publisher(state_root)?;
        let _ = publisher.publish(LexicalChannelOp::FullBundle(LexicalFullBundle {
            repo_id: repo(),
            revision_id: revision(),
            generation: generation(),
            payload: b"manifest".to_vec(),
        }))?;
        let _ = publisher.publish(LexicalChannelOp::UpsertChunk(UpsertChunk {
            repo_id: repo(),
            revision_id: revision(),
            generation: generation(),
            chunk_id: ChunkId::new("chunk-tree"),
            payload: chunk_payload_with_metadata("src/lib.rs", "rust", 1, 1, "fn main() {}")?,
        }))?;
        let tree = ParseTreeRecord {
            wire_version: 1,
            lang: LanguageCode::new("rust")
                .map_err(|err| -> Box<dyn Error> { format!("invalid tree lang: {err}").into() })?,
            root: ParseNode {
                kind: "function_item".to_string().into_boxed_str(),
                byte_start: 0,
                byte_end: 10,
                children: Vec::new(),
            },
            source_hash: compute_parse_tree_source_hash("fn main() {}"),
            role_tag_schema_version: 1,
            role_tags: Vec::new(),
        };
        let mut payload = Vec::new();
        ciborium::into_writer(&tree, &mut payload)
            .map_err(|err| -> Box<dyn Error> { format!("encode parse tree: {err}").into() })?;
        let _ = publisher.publish(LexicalChannelOp::UpsertParseTree(UpsertParseTree {
            repo_id: repo(),
            revision_id: revision(),
            generation: generation(),
            chunk_id: ChunkId::new("chunk-tree"),
            payload,
        }))?;
        let _ = publisher.publish(LexicalChannelOp::DeleteChunk(DeleteChunk {
            repo_id: repo(),
            revision_id: revision(),
            generation: generation(),
            chunk_id: ChunkId::new("chunk-tree"),
        }))?;
    }

    let config = build_config(state_root);
    let runtime = build_runtime(config)?;
    let socket = runtime.query_server.socket_path().to_path_buf();
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_for_drive = Arc::clone(&shutdown);
    let join = thread::Builder::new()
        .name("searchd-structural-shard-unavailable-test".into())
        .spawn(move || drive(runtime, shutdown_for_drive))?;

    if !wait_until(Duration::from_secs(2), || socket.exists()) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("socket never appeared".into());
    }

    let request = SearchPlaneQueryIpcRequestEnvelope {
        request_id: 44,
        payload: SearchPlaneQueryIpcRequest::Structural(StructuralQueryRequest {
            text_query: TextQueryRequest {
                syntax: TextQuerySyntax::Native,
                query_text: "match { :[x] }".to_string(),
                generation: Some(GenerationPin::new(repo(), revision(), generation())),
                generation_selector: None,
                top_k: 50,
            },
        }),
    };
    let mut observed: Option<String> = None;
    let saw_expected = wait_until(READINESS_TIMEOUT, || {
        match send_query_request(&socket, &request) {
            Ok(response) => match response.payload {
                SearchPlaneQueryIpcResponse::Error(err) => {
                    observed = Some(err.code.clone());
                    err.code == "STR_SHARD_UNAVAILABLE"
                }
                other => {
                    observed = Some(format!("{other:?}"));
                    false
                }
            },
            Err(err) => {
                observed = Some(format!("{err}"));
                false
            }
        }
    });
    if !saw_expected {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!(
            "expected STR_SHARD_UNAVAILABLE after orphaning structural chunk authority, observed {observed:?}"
        )
        .into());
    }

    shutdown.store(true, Ordering::Release);
    match join.join() {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(e.into()),
        Err(panic) => Err(format!("driver panic: {panic:?}").into()),
    }
}

/// Composition-wiring assertion (MINOR 6).
///
/// The structural producer wired in `searchd::app::runtime` must route
/// through the domain port and emit a typed error response — never a
/// panic, never a transport error, never a candidate list. The exact
/// error code is *loosely* asserted here so this gate survives Track 2's
/// current live producer adapter should return a typed readiness error when
/// no structural generation has been materialized yet.
/// The strict-code assertion lives in
/// `structural_query_returns_typed_generation_not_ready_error` above and
/// this looser wiring check ensures the composition root still emits a
/// typed structural code rather than panicking or returning a payload.
#[test]
fn structural_query_composition_wiring_emits_typed_error() -> TestResult {
    let dir = tempfile::tempdir()?;
    let state_root = dir.path();
    let config = build_config(state_root);
    let runtime = build_runtime(config)?;
    let socket = runtime.query_server.socket_path().to_path_buf();
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_for_drive = Arc::clone(&shutdown);
    let join = thread::Builder::new()
        .name("searchd-structural-wiring-test".into())
        .spawn(move || drive(runtime, shutdown_for_drive))?;

    if !wait_until(Duration::from_secs(2), || socket.exists()) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("socket never appeared".into());
    }

    let response = send_query_request(
        &socket,
        &SearchPlaneQueryIpcRequestEnvelope {
            request_id: 43,
            payload: SearchPlaneQueryIpcRequest::Structural(StructuralQueryRequest {
                text_query: TextQueryRequest {
                    syntax: TextQuerySyntax::Native,
                    query_text: "match { :[x] }".to_string(),
                    generation: Some(GenerationPin::new(repo(), revision(), generation())),
                    generation_selector: None,
                    top_k: 50,
                },
            }),
        },
    )?;

    let result: Result<(), String> = match response.payload {
        SearchPlaneQueryIpcResponse::Error(err) => {
            // Loose typed-code surface assertion: the dispatcher must
            // surface SOME structural-shaped typed error. Acceptable
            // shape today: STR_GENERATION_NOT_READY while no structural
            // materialization exists. Anything else is a genuine wiring
            // regression.
            let code = err.code.as_str();
            if code == "STR_GENERATION_NOT_READY" {
                Ok(())
            } else {
                Err(format!(
                    "structural composition wiring: expected typed structural error \
                     (STR_GENERATION_NOT_READY), got code={code} \
                     message={}",
                    err.message
                ))
            }
        }
        SearchPlaneQueryIpcResponse::Structural(_) => Err(
            "structural composition wiring: expected Error, got Structural \
                 (no structural generation was materialized for this test)"
                .to_string(),
        ),
        other => Err(format!(
            "structural composition wiring: expected Error response, got {other:?}"
        )),
    };

    shutdown.store(true, Ordering::Release);
    drop(join.join());
    result.map_err(Into::into)
}

#[test]
fn structural_sourcegraph_query_returns_match_after_parse_tree_ingest() -> TestResult {
    let dir = tempfile::tempdir()?;
    let state_root = dir.path();
    publish_structural_ready_fixture(state_root)?;

    let config = build_config(state_root);
    let runtime = build_runtime(config)?;
    let socket = runtime.query_server.socket_path().to_path_buf();
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_for_drive = Arc::clone(&shutdown);
    let join = thread::Builder::new()
        .name("searchd-structural-sourcegraph-success-test".into())
        .spawn(move || drive(runtime, shutdown_for_drive))?;

    if !wait_until(Duration::from_secs(2), || socket.exists()) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("socket never appeared".into());
    }

    let pin = GenerationPin::new(repo(), revision(), generation());
    let request = SearchPlaneQueryIpcRequestEnvelope {
        request_id: 46,
        payload: SearchPlaneQueryIpcRequest::Structural(StructuralQueryRequest {
            text_query: TextQueryRequest {
                syntax: TextQuerySyntax::Sourcegraph,
                query_text: r#"repo:repo-int path:src/lib.rs lang:rust patterntype:structural "function_item { { identifier :[name] } }""#.to_string(),
                generation: Some(pin.clone()),
                generation_selector: None,
                top_k: 50,
            },
        }),
    };
    let mut observed: Option<String> = None;
    let saw_ready = wait_until(READINESS_TIMEOUT, || {
        match send_query_request(&socket, &request) {
            Ok(response) => match response.payload {
                SearchPlaneQueryIpcResponse::Structural(structural) => {
                    observed = Some(format!("{structural:?}"));
                    structural.generation == pin && structural.results.len() == 1
                }
                SearchPlaneQueryIpcResponse::Error(err)
                    if err.code == "NOT_READY" || err.code == "STR_GENERATION_NOT_READY" =>
                {
                    observed = Some(err.code);
                    false
                }
                other => {
                    observed = Some(format!("{other:?}"));
                    false
                }
            },
            Err(err) => {
                observed = Some(err.to_string());
                false
            }
        }
    });
    if !saw_ready {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!(
            "structural Sourcegraph query never became ready; observed {observed:?}"
        )
        .into());
    }

    let response = send_query_request(&socket, &request)?;
    let structural = match response.payload {
        SearchPlaneQueryIpcResponse::Structural(structural) => structural,
        other => {
            shutdown.store(true, Ordering::Release);
            drop(join.join());
            return Err(format!("expected Structural, got {other:?}").into());
        }
    };
    if structural.generation != pin || structural.results.len() != 1 {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!("unexpected structural Sourcegraph response: {structural:?}").into());
    }
    let candidate = structural
        .results
        .first()
        .ok_or_else(|| "missing structural Sourcegraph candidate".to_string())?;
    let binding = candidate
        .bindings
        .first()
        .ok_or_else(|| "missing structural Sourcegraph binding".to_string())?;
    if candidate.candidate_id != "chunk-tree"
        || binding.metavariable != "name"
        || binding.start_byte != 3
        || binding.end_byte != 7
    {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(
            format!("unexpected structural Sourcegraph candidate/binding: {candidate:?}").into(),
        );
    }

    shutdown.store(true, Ordering::Release);
    match join.join() {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(e.into()),
        Err(panic) => Err(format!("driver panic: {panic:?}").into()),
    }
}

#[test]
fn structural_sourcegraph_query_requires_structural_pattern_type() -> TestResult {
    let dir = tempfile::tempdir()?;
    let state_root = dir.path();
    let config = build_config(state_root);
    let runtime = build_runtime(config)?;
    let socket = runtime.query_server.socket_path().to_path_buf();
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_for_drive = Arc::clone(&shutdown);
    let join = thread::Builder::new()
        .name("searchd-structural-sourcegraph-pattern-type-test".into())
        .spawn(move || drive(runtime, shutdown_for_drive))?;

    if !wait_until(Duration::from_secs(2), || socket.exists()) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("socket never appeared".into());
    }

    let response = send_query_request(
        &socket,
        &SearchPlaneQueryIpcRequestEnvelope {
            request_id: 47,
            payload: SearchPlaneQueryIpcRequest::Structural(StructuralQueryRequest {
                text_query: TextQueryRequest {
                    syntax: TextQuerySyntax::Sourcegraph,
                    query_text: r#""function_item""#.to_string(),
                    generation: Some(GenerationPin::new(repo(), revision(), generation())),
                    generation_selector: None,
                    top_k: 50,
                },
            }),
        },
    )?;
    let err = match response.payload {
        SearchPlaneQueryIpcResponse::Error(err) => err,
        other => {
            shutdown.store(true, Ordering::Release);
            drop(join.join());
            return Err(format!("expected Error, got {other:?}").into());
        }
    };
    if err.code != "BRIDGE_TRANSLATE_FAIL" || !err.message.contains("patterntype:structural") {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!(
            "expected BRIDGE_TRANSLATE_FAIL structural pattern-type error, got {err:?}"
        )
        .into());
    }

    shutdown.store(true, Ordering::Release);
    match join.join() {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(e.into()),
        Err(panic) => Err(format!("driver panic: {panic:?}").into()),
    }
}

#[test]
fn structural_sourcegraph_query_rejects_select_filter() -> TestResult {
    let dir = tempfile::tempdir()?;
    let state_root = dir.path();
    publish_structural_ready_fixture(state_root)?;
    let config = build_config(state_root);
    let runtime = build_runtime(config)?;
    let socket = runtime.query_server.socket_path().to_path_buf();
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_for_drive = Arc::clone(&shutdown);
    let join = thread::Builder::new()
        .name("searchd-structural-sourcegraph-select-filter-test".into())
        .spawn(move || drive(runtime, shutdown_for_drive))?;

    if !wait_until(Duration::from_secs(2), || socket.exists()) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("socket never appeared".into());
    }

    let request = SearchPlaneQueryIpcRequestEnvelope {
        request_id: 48,
        payload: SearchPlaneQueryIpcRequest::Structural(StructuralQueryRequest {
            text_query: TextQueryRequest {
                syntax: TextQuerySyntax::Sourcegraph,
                query_text: r#"select:repo patterntype:structural "function_item""#.to_string(),
                generation: Some(GenerationPin::new(repo(), revision(), generation())),
                generation_selector: None,
                top_k: 50,
            },
        }),
    };
    let mut observed: Option<String> = None;
    let saw_expected = wait_until(READINESS_TIMEOUT, || {
        match send_query_request(&socket, &request) {
            Ok(response) => match response.payload {
                SearchPlaneQueryIpcResponse::Error(err)
                    if err.code == "NOT_READY" || err.code == "STR_GENERATION_NOT_READY" =>
                {
                    observed = Some(err.code);
                    false
                }
                SearchPlaneQueryIpcResponse::Error(err) => {
                    observed = Some(format!("{err:?}"));
                    err.code == "STR_INVALID_REQUEST" && err.message.contains("filter `select`")
                }
                other => {
                    observed = Some(format!("{other:?}"));
                    false
                }
            },
            Err(err) => {
                observed = Some(err.to_string());
                false
            }
        }
    });
    if !saw_expected {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!(
            "expected STR_INVALID_REQUEST for structural SG select filter, observed {observed:?}"
        )
        .into());
    }

    shutdown.store(true, Ordering::Release);
    match join.join() {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(e.into()),
        Err(panic) => Err(format!("driver panic: {panic:?}").into()),
    }
}

#[test]
fn bridge_query_sourcegraph_returns_packet_with_candidates_and_metadata() -> TestResult {
    let dir = tempfile::tempdir()?;
    let state_root = dir.path();
    let pin = GenerationPin::new(repo(), revision(), generation());

    {
        let publisher = open_lexical_publisher(state_root)?;
        let _ = publisher.publish(LexicalChannelOp::FullBundle(LexicalFullBundle {
            repo_id: repo(),
            revision_id: revision(),
            generation: generation(),
            payload: b"manifest".to_vec(),
        }))?;
        let _ = publisher.publish(LexicalChannelOp::UpsertChunk(UpsertChunk {
            repo_id: repo(),
            revision_id: revision(),
            generation: generation(),
            chunk_id: ChunkId::new("bridge-alpha"),
            payload: chunk_payload("sphinx of quartz")?,
        }))?;
        let _ = publisher.publish(LexicalChannelOp::UpsertChunk(UpsertChunk {
            repo_id: repo(),
            revision_id: revision(),
            generation: generation(),
            chunk_id: ChunkId::new("bridge-beta"),
            payload: chunk_payload("other content")?,
        }))?;
        let _ = publisher.seal(repo(), revision(), generation())?;
    }

    let config = build_config(state_root);
    let runtime = build_runtime(config)?;
    let socket = runtime.query_server.socket_path().to_path_buf();
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_for_drive = Arc::clone(&shutdown);
    let join = thread::Builder::new()
        .name("searchd-bridge-test".into())
        .spawn(move || drive(runtime, shutdown_for_drive))?;

    if !wait_until(Duration::from_secs(2), || socket.exists()) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("socket never appeared".into());
    }
    if !wait_until(READINESS_TIMEOUT, || {
        send_query_request(&socket, &lex_query("sphinx"))
            .map(|resp| !matches!(resp.payload, SearchPlaneQueryIpcResponse::Error(_)))
            .unwrap_or(false)
    }) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("dispatcher never sealed lexical bridge generation".into());
    }

    let query_text = "repo:repo-int sphinx".to_string();
    let response = send_query_request(
        &socket,
        &SearchPlaneQueryIpcRequestEnvelope {
            request_id: 43,
            payload: SearchPlaneQueryIpcRequest::Bridge(BridgeQueryRequest {
                text_query: TextQueryRequest {
                    syntax: TextQuerySyntax::Sourcegraph,
                    query_text: query_text.clone(),
                    generation: Some(pin.clone()),
                    generation_selector: None,
                    top_k: 50,
                },
                target: BridgeTarget::CodeQl,
            }),
        },
    )?;
    let bridge = match response.payload {
        SearchPlaneQueryIpcResponse::Bridge(bridge) => bridge,
        other => {
            shutdown.store(true, Ordering::Release);
            drop(join.join());
            return Err(format!("expected Bridge, got {other:?}").into());
        }
    };
    if bridge.generation != pin {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("bridge response generation did not echo request pin".into());
    }
    if bridge.packet.scope != BridgeScope::Lexical {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!(
            "expected lexical bridge scope, got {:?}",
            bridge.packet.scope
        )
        .into());
    }
    if bridge.packet.source_syntax.as_deref() != Some(query_text.as_str()) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!(
            "expected Sourcegraph source_syntax {:?}, got {:?}",
            query_text, bridge.packet.source_syntax
        )
        .into());
    }
    if bridge.packet.translator_version.as_deref() != Some(TRANSLATOR_VERSION) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!(
            "expected translator_version {}, got {:?}",
            TRANSLATOR_VERSION, bridge.packet.translator_version
        )
        .into());
    }
    let ids: Vec<String> = bridge
        .packet
        .candidates
        .iter()
        .map(|candidate| candidate.candidate_id.clone())
        .collect();
    if !ids.iter().any(|id| id == "bridge-alpha") {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!("expected bridge-alpha candidate in packet, got {ids:?}").into());
    }

    shutdown.store(true, Ordering::Release);
    match join.join() {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(e.into()),
        Err(panic) => Err(format!("driver panic: {panic:?}").into()),
    }
}

fn lex_query(needle: &str) -> SearchPlaneQueryIpcRequestEnvelope {
    SearchPlaneQueryIpcRequestEnvelope {
        request_id: 0,
        payload: SearchPlaneQueryIpcRequest::Text(TextQueryRequest {
            syntax: TextQuerySyntax::Sourcegraph,
            query_text: needle.to_string(),
            generation: Some(GenerationPin::new(repo(), revision(), generation())),
            generation_selector: None,
            top_k: 50,
        }),
    }
}

fn history_query(query_text: &str) -> SearchPlaneQueryIpcRequestEnvelope {
    SearchPlaneQueryIpcRequestEnvelope {
        request_id: 0,
        payload: SearchPlaneQueryIpcRequest::History(HistoryQueryRequest {
            text_query: TextQueryRequest {
                syntax: TextQuerySyntax::Sourcegraph,
                query_text: query_text.to_string(),
                generation: Some(GenerationPin::new(repo(), revision(), generation())),
                generation_selector: None,
                top_k: 50,
            },
        }),
    }
}

fn wait_for_typed_error(
    socket: &Path,
    request: &SearchPlaneQueryIpcRequestEnvelope,
    timeout: Duration,
) -> Result<quanta_index_contract::SearchPlaneIpcError, Box<dyn Error>> {
    let mut last_observed = String::new();
    if !wait_until(timeout, || match send_query_request(socket, request) {
        Ok(response) => match response.payload {
            SearchPlaneQueryIpcResponse::Error(err) => {
                last_observed = err.code;
                true
            }
            other => {
                last_observed = format!("{other:?}");
                false
            }
        },
        Err(err) => {
            last_observed = err.to_string();
            false
        }
    }) {
        return Err(format!("typed error never surfaced before timeout: {last_observed}").into());
    }
    let response = send_query_request(socket, request)?;
    match response.payload {
        SearchPlaneQueryIpcResponse::Error(err) => Ok(err),
        other => Err(format!("expected Error response after readiness wait, got {other:?}").into()),
    }
}
