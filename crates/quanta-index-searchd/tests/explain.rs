//! Explain query path: validates that explain reports presence-in-index for a
//! candidate previously returned by a lexical query.

#![forbid(unsafe_code)]

use std::error::Error;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use quanta_index_channel::{BundleChannelPublisher, open_lexical_publisher};
use quanta_index_contract::{
    ChunkId, GenerationPin, LexicalCandidate, LexicalChannelOp, LqDirectiveSet, LqExpr,
    LqFilterSet, LqOptionSet, LqQuery, ManifestGeneration, RepoId, RepoRelativePath, RevisionId,
    SearchPlaneExplainQueryRequest, SearchPlaneIpcRequest, SearchPlaneIpcRequestEnvelope,
    SearchPlaneIpcResponse, SearchPlaneLexicalQueryRequest, UpsertChunk,
};
use quanta_index_ipc::send_request;
use quanta_index_searchd::app::SearchdConfig;
use quanta_index_searchd::app::runtime::SearchdRuntime;
use quanta_index_searchd::app::searchd::drive;

type TestResult = Result<(), Box<dyn Error>>;

fn repo() -> RepoId {
    RepoId::new("repo-exp")
}

fn revision() -> RevisionId {
    RevisionId::new("rev-exp")
}

fn generation() -> ManifestGeneration {
    ManifestGeneration::new(11)
}

fn unique_socket_path() -> std::path::PathBuf {
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!("qi-explain-{pid}-{nanos}.sock"))
}

fn build_config(state_root: &Path) -> SearchdConfig {
    let mut cfg = SearchdConfig::from_state_root(state_root.to_path_buf());
    cfg = SearchdConfig::with_socket_override(cfg, unique_socket_path());
    cfg
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

fn make_lq(expr: LqExpr) -> LqQuery {
    LqQuery {
        expr,
        filters: LqFilterSet::default(),
        options: LqOptionSet::default(),
        directives: LqDirectiveSet::default(),
    }
}

fn lex_query(needle: &str, pin: GenerationPin) -> SearchPlaneIpcRequestEnvelope {
    SearchPlaneIpcRequestEnvelope {
        request_id: 1,
        payload: SearchPlaneIpcRequest::Lexical(SearchPlaneLexicalQueryRequest {
            query: make_lq(LqExpr::Raw(needle.to_string())),
            generation: Some(pin),
        }),
    }
}

fn explain_request(
    pin: GenerationPin,
    candidate: LexicalCandidate,
) -> SearchPlaneIpcRequestEnvelope {
    SearchPlaneIpcRequestEnvelope {
        request_id: 2,
        payload: SearchPlaneIpcRequest::Explain(SearchPlaneExplainQueryRequest {
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
            payload: b"quick brown fox jumps".to_vec(),
        }))?;
        let _ = publisher.seal(repo(), revision(), generation())?;
    }

    let config = build_config(state_root);
    let runtime = SearchdRuntime::build(config)?;
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
    if !wait_until(Duration::from_secs(2), || {
        send_request(&socket, &lex_query("quick", pin.clone()))
            .map(|r| !matches!(r.payload, SearchPlaneIpcResponse::Error(_)))
            .unwrap_or(false)
    }) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("dispatcher never sealed".into());
    }

    let lex_resp = send_request(&socket, &lex_query("quick", pin.clone()))?;
    let candidate = match lex_resp.payload {
        SearchPlaneIpcResponse::Lexical(lex) => lex
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

    let explain_resp = send_request(&socket, &explain_request(pin.clone(), candidate.clone()))?;
    let explanation = match explain_resp.payload {
        SearchPlaneIpcResponse::Explain(exp) => exp.explanation,
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
            payload: b"alpha bravo charlie".to_vec(),
        }))?;
        let _ = publisher.seal(repo(), revision(), generation())?;
    }

    let config = build_config(state_root);
    let runtime = SearchdRuntime::build(config)?;
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
    if !wait_until(Duration::from_secs(2), || {
        send_request(&socket, &lex_query("alpha", pin.clone()))
            .map(|r| !matches!(r.payload, SearchPlaneIpcResponse::Error(_)))
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
    let resp = send_request(&socket, &explain_request(pin, stale_candidate))?;
    let err = match resp.payload {
        SearchPlaneIpcResponse::Error(e) => e,
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
