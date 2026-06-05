//! Explain query path: validates that explain reports presence-in-index for a
//! candidate previously returned by a lexical query.

#![forbid(unsafe_code)]
#![expect(
    clippy::disallowed_methods,
    reason = "test polling paths still use explicit Result fallback checks"
)]
#![expect(
    clippy::wildcard_enum_match_arm,
    reason = "integration response checks intentionally collapse non-target variants"
)]

use std::error::Error;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use quanta_index_contract::lex::LanguageCode;
use quanta_index_contract::{
    BatchIngestMode, ChunkId, ChunkRecord, GenerationPin, LexicalCandidate, LexicalIngestBatch,
    LexicalReplaceScope, ManifestGeneration, RepoId, RepoRelativePath, RevisionId,
    SearchPlaneExplainQueryRequest, SearchPlaneIngestIpcRequest,
    SearchPlaneIngestIpcRequestEnvelope, SearchPlaneIngestIpcResponse,
    SearchPlaneIngestIpcResponseEnvelope, SearchPlaneQueryIpcRequest,
    SearchPlaneQueryIpcRequestEnvelope, SearchPlaneQueryIpcResponse,
    SearchPlaneQueryIpcResponseEnvelope, SearchScopeKey, SearchScopeSurface, TextQueryRequest,
    TextQuerySyntax,
};
use quanta_index_ipc::send_request;
use quanta_index_searchd::app::SearchdConfig;
use quanta_index_searchd::app::searchd::drive;
use quanta_index_searchd_runtime::build_runtime;

type TestResult = Result<(), Box<dyn Error>>;
type RuntimeHandles = (
    PathBuf,
    PathBuf,
    Arc<AtomicBool>,
    thread::JoinHandle<anyhow::Result<()>>,
);
static NEXT_SOCKET_ID: AtomicU64 = AtomicU64::new(0);
const READINESS_TIMEOUT: Duration = Duration::from_secs(5);
const SOCKET_APPEAR_TIMEOUT: Duration = Duration::from_secs(5);

fn repo() -> RepoId {
    RepoId::new("repo-exp")
}

fn revision() -> RevisionId {
    RevisionId::new("rev-exp")
}

fn generation() -> ManifestGeneration {
    ManifestGeneration::new(11)
}

fn chunk_record(id: &str, text: &str) -> Result<ChunkRecord, Box<dyn Error>> {
    Ok(ChunkRecord {
        chunk_id: ChunkId::new(id),
        repo_relative_path: RepoRelativePath::new("src/explain.txt"),
        language: LanguageCode::new("text")
            .map_err(|err| -> Box<dyn Error> { format!("invalid language code: {err}").into() })?,
        start_byte: 0,
        end_byte: u32::try_from(text.len()).map_err(|err| -> Box<dyn Error> {
            format!("chunk text length overflow: {err}").into()
        })?,
        start_line: 0,
        end_line: 0,
        text: text.to_string().into_boxed_str(),
        structural: None,
        parent_chunk_id: None,
        source_repo_id: None,
    })
}

fn unique_socket_paths() -> (std::path::PathBuf, std::path::PathBuf, std::path::PathBuf) {
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let sequence = NEXT_SOCKET_ID.fetch_add(1, Ordering::Relaxed);
    let query =
        std::env::temp_dir().join(format!("qi-explain-query-{pid}-{nanos}-{sequence}.sock"));
    let control =
        std::env::temp_dir().join(format!("qi-explain-control-{pid}-{nanos}-{sequence}.sock"));
    let ingest =
        std::env::temp_dir().join(format!("qi-explain-ingest-{pid}-{nanos}-{sequence}.sock"));
    (query, control, ingest)
}

fn build_config(state_root: &Path) -> SearchdConfig {
    let mut cfg = SearchdConfig::from_state_root(state_root.to_path_buf());
    let (query_socket, control_socket, ingest_socket) = unique_socket_paths();
    cfg = SearchdConfig::with_socket_overrides(cfg, query_socket, control_socket);
    SearchdConfig::with_ingest_socket_override(cfg, ingest_socket)
}

fn send_query_request(
    socket: &Path,
    request: &SearchPlaneQueryIpcRequestEnvelope,
) -> Result<SearchPlaneQueryIpcResponseEnvelope, quanta_index_ipc::IpcError> {
    send_request(socket, request)
}

fn send_ingest_request(
    socket: &Path,
    request: &SearchPlaneIngestIpcRequestEnvelope,
) -> Result<SearchPlaneIngestIpcResponseEnvelope, quanta_index_ipc::IpcError> {
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

fn lex_query(needle: &str, pin: GenerationPin) -> SearchPlaneQueryIpcRequestEnvelope {
    SearchPlaneQueryIpcRequestEnvelope {
        request_id: 1,
        payload: SearchPlaneQueryIpcRequest::Text(TextQueryRequest {
            syntax: TextQuerySyntax::Sourcegraph,
            query_text: needle.to_string(),
            generation: Some(pin),
            generation_selector: None,
            top_k: 50,
        }),
    }
}

fn explain_request(
    pin: GenerationPin,
    candidate: LexicalCandidate,
) -> SearchPlaneQueryIpcRequestEnvelope {
    SearchPlaneQueryIpcRequestEnvelope {
        request_id: 2,
        payload: SearchPlaneQueryIpcRequest::Explain(SearchPlaneExplainQueryRequest {
            generation: pin,
            candidate,
        }),
    }
}

fn scope_key(path: &str) -> SearchScopeKey {
    SearchScopeKey {
        doc_surface: SearchScopeSurface::Chunk,
        repo_relative_path: RepoRelativePath::new(path),
    }
}

fn start_runtime(state_root: &Path, thread_name: &str) -> Result<RuntimeHandles, Box<dyn Error>> {
    let config = build_config(state_root);
    let runtime = build_runtime(config)?;
    let query_socket = runtime.query_server.socket_path().to_path_buf();
    let ingest_socket = runtime.ingest_server.socket_path().to_path_buf();
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_for_drive = Arc::clone(&shutdown);
    let join = thread::Builder::new()
        .name(thread_name.into())
        .spawn(move || drive(runtime, &shutdown_for_drive))?;
    if !wait_until(SOCKET_APPEAR_TIMEOUT, || {
        query_socket.exists() && ingest_socket.exists()
    }) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!(
            "sockets never appeared query={} ingest={}",
            query_socket.display(),
            ingest_socket.display()
        )
        .into());
    }
    Ok((query_socket, ingest_socket, shutdown, join))
}

fn dispatch_ingest(socket: &Path, payload: SearchPlaneIngestIpcRequest) -> TestResult {
    let response = send_ingest_request(
        socket,
        &SearchPlaneIngestIpcRequestEnvelope {
            request_id: NEXT_SOCKET_ID.fetch_add(1, Ordering::Relaxed),
            payload,
        },
    )?;
    match response.payload {
        SearchPlaneIngestIpcResponse::Error(err) => {
            Err(format!("ingest failed code={} message={}", err.code, err.message).into())
        }
        _ => Ok(()),
    }
}

fn publish_chunk(socket: &Path, chunk: ChunkRecord) -> TestResult {
    dispatch_ingest(
        socket,
        SearchPlaneIngestIpcRequest::PublishLexicalBatch(LexicalIngestBatch {
            repo_id: repo(),
            revision_id: revision(),
            generation: generation(),
            base_generation: None,
            manifest_digest: format!("explain-lex-manifest-{}", generation().get()),
            batch_digest: format!(
                "explain-lex-batch-{}",
                NEXT_SOCKET_ID.fetch_add(1, Ordering::Relaxed)
            ),
            mode: BatchIngestMode::ReplaceGeneration,
            bundle_payload: None,
            replace_scopes: vec![LexicalReplaceScope {
                scope: scope_key(chunk.repo_relative_path.as_str()),
                scope_digest: "explain-scope".to_string(),
                chunks: vec![chunk],
                symbols: Vec::new(),
            }],
            tombstone_scopes: Vec::new(),
            seal: false,
        }),
    )
}

fn seal_lexical(socket: &Path) -> TestResult {
    dispatch_ingest(
        socket,
        SearchPlaneIngestIpcRequest::PublishLexicalBatch(LexicalIngestBatch {
            repo_id: repo(),
            revision_id: revision(),
            generation: generation(),
            base_generation: None,
            manifest_digest: format!("explain-lex-seal-{}", generation().get()),
            batch_digest: format!(
                "explain-lex-seal-batch-{}",
                NEXT_SOCKET_ID.fetch_add(1, Ordering::Relaxed)
            ),
            mode: BatchIngestMode::ReplaceGeneration,
            bundle_payload: None,
            replace_scopes: Vec::new(),
            tombstone_scopes: Vec::new(),
            seal: true,
        }),
    )
}

#[test]
fn explain_reports_present_candidate() -> TestResult {
    let dir = tempfile::tempdir()?;
    let state_root = dir.path();
    let (socket, ingest_socket, shutdown, join) = start_runtime(state_root, "explain-test")?;
    publish_chunk(
        &ingest_socket,
        chunk_record("explain-c1", "quick brown fox jumps")?,
    )?;
    seal_lexical(&ingest_socket)?;

    let pin = GenerationPin::new(repo(), revision(), generation());
    if !wait_until(READINESS_TIMEOUT, || {
        send_query_request(&socket, &lex_query("quick", pin.clone()))
            .map(|r| !matches!(r.payload, SearchPlaneQueryIpcResponse::Error(_)))
            .unwrap_or(false)
    }) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("dispatcher never sealed".into());
    }

    let lex_resp = send_query_request(&socket, &lex_query("quick", pin.clone()))?;
    let candidate = match lex_resp.payload {
        SearchPlaneQueryIpcResponse::Text(lex) => lex
            .results
            .into_iter()
            .next()
            .ok_or_else(|| Box::<dyn Error>::from("lexical query returned zero candidates"))?,
        other => {
            shutdown.store(true, Ordering::Release);
            drop(join.join());
            return Err(format!("expected Lexical, got {other:?}").into());
        }
    };
    if candidate.candidate_id != "explain-c1" {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!("expected explain-c1, got {}", candidate.candidate_id).into());
    }

    let explain_resp = send_query_request(&socket, &explain_request(pin, candidate))?;
    let explanation = match explain_resp.payload {
        SearchPlaneQueryIpcResponse::Explain(exp) => exp.explanation,
        other => {
            shutdown.store(true, Ordering::Release);
            drop(join.join());
            return Err(format!("expected Explain, got {other:?}").into());
        }
    };
    if !explanation.summary.contains("present") {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!(
            "expected 'present' in explanation summary, got: {}",
            explanation.summary
        )
        .into());
    }
    if !explanation.summary.contains("explain-c1") {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!(
            "expected candidate id in summary, got: {}",
            explanation.summary
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
fn explain_rejects_generation_mismatch() -> TestResult {
    let dir = tempfile::tempdir()?;
    let state_root = dir.path();
    let (socket, ingest_socket, shutdown, join) =
        start_runtime(state_root, "explain-mismatch-test")?;
    publish_chunk(
        &ingest_socket,
        chunk_record("c-mismatch", "alpha bravo charlie")?,
    )?;
    seal_lexical(&ingest_socket)?;

    let pin = GenerationPin::new(repo(), revision(), generation());
    if !wait_until(READINESS_TIMEOUT, || {
        send_query_request(&socket, &lex_query("alpha", pin.clone()))
            .map(|r| !matches!(r.payload, SearchPlaneQueryIpcResponse::Error(_)))
            .unwrap_or(false)
    }) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("dispatcher never sealed".into());
    }

    // Construct a candidate whose manifest_generation differs from the pin.
    let stale_candidate = LexicalCandidate {
        candidate_id: "c-mismatch".to_string(),
        repo_id: repo(),
        revision_id: revision(),
        manifest_generation: ManifestGeneration::new(99),
        repo_relative_path: RepoRelativePath::new(""),
        start_line: 0,
        end_line: 0,
        score: 1.0,
        snippet: "alpha bravo charlie".to_string(),
    };
    let resp = send_query_request(&socket, &explain_request(pin, stale_candidate))?;
    let err = match resp.payload {
        SearchPlaneQueryIpcResponse::Error(e) => e,
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

    shutdown.store(true, Ordering::Release);
    match join.join() {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(e.into()),
        Err(panic) => Err(format!("driver panic: {panic:?}").into()),
    }
}
