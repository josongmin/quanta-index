//! Explain query path: validates that explain reports presence-in-index for a
//! candidate previously returned by a lexical query.

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

use quanta_index_channel::{BundleChannelPublisher, open_lexical_publisher};
use quanta_index_contract::{
    ChunkId, ChunkRecord, GenerationPin, LexicalCandidate, LexicalChannelOp, ManifestGeneration,
    RepoId, RepoRelativePath, RevisionId, SearchPlaneExplainQueryRequest,
    SearchPlaneQueryIpcRequest, SearchPlaneQueryIpcRequestEnvelope, SearchPlaneQueryIpcResponse,
    SearchPlaneQueryIpcResponseEnvelope, TextQueryRequest, TextQuerySyntax, UpsertChunk,
};
use quanta_index_ipc::send_request;
use quanta_index_searchd::app::SearchdConfig;
use quanta_index_searchd::app::searchd::drive;
use quanta_index_searchd_runtime::build_runtime;

type TestResult = Result<(), Box<dyn Error>>;
static NEXT_SOCKET_ID: AtomicU64 = AtomicU64::new(0);
const READINESS_TIMEOUT: Duration = Duration::from_secs(5);

fn repo() -> RepoId {
    RepoId::new("repo-exp")
}

fn revision() -> RevisionId {
    RevisionId::new("rev-exp")
}

fn generation() -> ManifestGeneration {
    ManifestGeneration::new(11)
}

fn chunk_payload(text: &str) -> Result<Vec<u8>, Box<dyn Error>> {
    let record = ChunkRecord {
        repo_relative_path: RepoRelativePath::new(""),
        language: String::new().into_boxed_str(),
        start_line: 0,
        end_line: 0,
        snippet: text.to_string().into_boxed_str(),
    };
    let mut buf = Vec::new();
    ciborium::into_writer(&record, &mut buf)
        .map_err(|err| -> Box<dyn Error> { format!("encode chunk: {err}").into() })?;
    Ok(buf)
}

fn unique_socket_paths() -> (std::path::PathBuf, std::path::PathBuf) {
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
    (query, control)
}

fn build_config(state_root: &Path) -> SearchdConfig {
    let mut cfg = SearchdConfig::from_state_root(state_root.to_path_buf());
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

fn lex_query(needle: &str, pin: GenerationPin) -> SearchPlaneQueryIpcRequestEnvelope {
    SearchPlaneQueryIpcRequestEnvelope {
        request_id: 1,
        payload: SearchPlaneQueryIpcRequest::Text(TextQueryRequest {
            syntax: TextQuerySyntax::Sourcegraph,
            query_text: needle.to_string(),
            generation: Some(pin),
            generation_selector: None,
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

#[test]
fn explain_reports_present_candidate() -> TestResult {
    let dir = tempfile::tempdir()?;
    let state_root = dir.path();

    {
        let publisher = open_lexical_publisher(state_root)?;
        let _ = publisher.publish(LexicalChannelOp::UpsertChunk(UpsertChunk {
            repo_id: repo(),
            revision_id: revision(),
            generation: generation(),
            chunk_id: ChunkId::new("explain-c1"),
            payload: chunk_payload("quick brown fox jumps")?,
        }))?;
        let _ = publisher.seal(repo(), revision(), generation())?;
    }

    let config = build_config(state_root);
    let runtime = build_runtime(config)?;
    let socket = runtime.query_server.socket_path().to_path_buf();
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_for_drive = Arc::clone(&shutdown);
    let join = thread::Builder::new()
        .name("explain-test".into())
        .spawn(move || drive(runtime, shutdown_for_drive))?;

    let pin = GenerationPin::new(repo(), revision(), generation());
    if !wait_until(Duration::from_secs(2), || socket.exists()) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("socket never appeared".into());
    }
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

    {
        let publisher = open_lexical_publisher(state_root)?;
        let _ = publisher.publish(LexicalChannelOp::UpsertChunk(UpsertChunk {
            repo_id: repo(),
            revision_id: revision(),
            generation: generation(),
            chunk_id: ChunkId::new("c-mismatch"),
            payload: chunk_payload("alpha bravo charlie")?,
        }))?;
        let _ = publisher.seal(repo(), revision(), generation())?;
    }

    let config = build_config(state_root);
    let runtime = build_runtime(config)?;
    let socket = runtime.query_server.socket_path().to_path_buf();
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_for_drive = Arc::clone(&shutdown);
    let join = thread::Builder::new()
        .name("explain-mismatch-test".into())
        .spawn(move || drive(runtime, shutdown_for_drive))?;

    let pin = GenerationPin::new(repo(), revision(), generation());
    if !wait_until(Duration::from_secs(2), || socket.exists()) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("socket never appeared".into());
    }
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
