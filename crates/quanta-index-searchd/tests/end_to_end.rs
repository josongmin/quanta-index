//! End-to-end integration: publisher writes ops → searchd dispatcher consumes
//! and feeds adapters → searchd UDS query server returns matches.

#![forbid(unsafe_code)]

use std::error::Error;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use quanta_index_channel::{
    BundleChannelPublisher, open_lexical_publisher, open_semantic_publisher,
};
use quanta_index_contract::{
    ChunkId, EmbeddingId, GenerationPin, LexicalChannelOp, LexicalFullBundle, LqDirectiveSet,
    LqExpr, LqFilterSet, LqOptionSet, LqQuery, ManifestGeneration, RepoId, RevisionId,
    SearchPlaneHybridQueryRequest, SearchPlaneIpcRequest, SearchPlaneIpcRequestEnvelope,
    SearchPlaneIpcResponse, SearchPlaneLexicalQueryRequest, SearchPlaneSemanticQueryRequest,
    SemanticChannelOp, SemanticFullBundle, UpsertChunk, UpsertEmbedding,
};
use quanta_index_ipc::send_request;
use quanta_index_searchd::app::SearchdConfig;
use quanta_index_searchd::app::runtime::SearchdRuntime;
use quanta_index_searchd::app::searchd::drive;

type TestResult = Result<(), Box<dyn Error>>;

fn repo() -> RepoId {
    RepoId::new("repo-int")
}

fn revision() -> RevisionId {
    RevisionId::new("rev-int")
}

fn generation() -> ManifestGeneration {
    ManifestGeneration::new(7)
}

fn unique_socket_path() -> std::path::PathBuf {
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!("qi-test-{pid}-{nanos}.sock"))
}

fn build_config(state_root: &Path) -> SearchdConfig {
    let mut cfg = SearchdConfig::from_state_root(state_root.to_path_buf());
    // The unit socket path under tmpdir state root can exceed the 104-byte
    // AF_UNIX limit on macOS for long temp paths; use a flat path in
    // /tmp instead.
    let socket = unique_socket_path();
    cfg = SearchdConfig::with_socket_override(cfg, socket);
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

fn float_vec_to_bytes(vec: &[f32]) -> Result<Vec<u8>, Box<dyn Error>> {
    let owned: Vec<f32> = vec.to_vec();
    let mut out: Vec<u8> = Vec::new();
    ciborium::into_writer(&owned, &mut out).map_err(|err| -> Box<dyn Error> {
        format!("ciborium encode embedding: {err}").into()
    })?;
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
            payload: b"hello world".to_vec(),
        }))?;
        let _ = publisher.publish(LexicalChannelOp::UpsertChunk(UpsertChunk {
            repo_id: repo(),
            revision_id: revision(),
            generation: generation(),
            chunk_id: ChunkId::new("c2"),
            payload: b"hello rust".to_vec(),
        }))?;
        let _ = publisher.publish(LexicalChannelOp::UpsertChunk(UpsertChunk {
            repo_id: repo(),
            revision_id: revision(),
            generation: generation(),
            chunk_id: ChunkId::new("c3"),
            payload: b"goodbye".to_vec(),
        }))?;
        let _ = publisher.seal(repo(), revision(), generation())?;
    }

    let config = build_config(state_root);
    let runtime = SearchdRuntime::build(config)?;
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
    // Wait for dispatcher to consume the seal.
    if !wait_until(Duration::from_secs(2), || {
        // Probe: send a lexical query and check it doesn't return NOT_READY.
        let probe = lex_query("hello");
        match send_request(&socket, &probe) {
            Ok(resp) => !matches!(resp.payload, SearchPlaneIpcResponse::Error(_)),
            Err(_) => false,
        }
    }) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("dispatcher never sealed generation".into());
    }

    let response = send_request(&socket, &lex_query("hello"))?;
    let candidates = match response.payload {
        SearchPlaneIpcResponse::Lexical(lex) => lex.results,
        other => return Err(format!("expected lexical response, got {other:?}").into()),
    };
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
            payload: b"only lex sealed".to_vec(),
        }))?;
        let _ = publisher.seal(repo(), revision(), generation())?;
    }

    let config = build_config(state_root);
    let runtime = SearchdRuntime::build(config)?;
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
    if !wait_until(Duration::from_secs(2), || {
        let probe = lex_query("only");
        send_request(&socket, &probe)
            .map(|r| !matches!(r.payload, SearchPlaneIpcResponse::Error(_)))
            .unwrap_or(false)
    }) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("lex seal not consumed".into());
    }

    // Hybrid query should fail NOT_READY because semantic side is unsealed.
    let hybrid_req = SearchPlaneIpcRequestEnvelope {
        request_id: 1,
        payload: SearchPlaneIpcRequest::Hybrid(SearchPlaneHybridQueryRequest {
            lexical_query: make_lq(LqExpr::Raw("only".to_string())),
            semantic_query_text: "1.0 0.0".to_string(),
            generation: Some(GenerationPin::new(repo(), revision(), generation())),
            top_k: 5,
        }),
    };
    let response = send_request(&socket, &hybrid_req)?;
    let err = match response.payload {
        SearchPlaneIpcResponse::Error(err) => err,
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
            payload: b"sphinx of quartz".to_vec(),
        }))?;
        let _ = lex_pub.publish(LexicalChannelOp::UpsertChunk(UpsertChunk {
            repo_id: repo(),
            revision_id: revision(),
            generation: generation(),
            chunk_id: ChunkId::new("beta"),
            payload: b"sphinx riddles".to_vec(),
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
    let runtime = SearchdRuntime::build(config)?;
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
    if !wait_until(Duration::from_secs(3), || {
        let req = SearchPlaneIpcRequestEnvelope {
            request_id: 0,
            payload: SearchPlaneIpcRequest::Hybrid(SearchPlaneHybridQueryRequest {
                lexical_query: make_lq(LqExpr::Raw("sphinx".to_string())),
                semantic_query_text: float_vec_to_query_text(&[1.0_f32, 0.0_f32]),
                generation: Some(pin.clone()),
                top_k: 5,
            }),
        };
        send_request(&socket, &req)
            .map(|r| !matches!(r.payload, SearchPlaneIpcResponse::Error(_)))
            .unwrap_or(false)
    }) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("joint seal never reached".into());
    }

    // Final hybrid: must return both candidates fused with alpha ranked first
    // (alpha matches both lexical substring AND nearest semantic vector to
    // [1,0]).
    let req = SearchPlaneIpcRequestEnvelope {
        request_id: 99,
        payload: SearchPlaneIpcRequest::Hybrid(SearchPlaneHybridQueryRequest {
            lexical_query: make_lq(LqExpr::Raw("sphinx".to_string())),
            semantic_query_text: float_vec_to_query_text(&[1.0_f32, 0.0_f32]),
            generation: Some(pin),
            top_k: 5,
        }),
    };
    let response = send_request(&socket, &req)?;
    let candidates = match response.payload {
        SearchPlaneIpcResponse::Hybrid(h) => h.results,
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
fn semantic_only_query_requires_semantic_seal() -> TestResult {
    let dir = tempfile::tempdir()?;
    let state_root = dir.path();
    let config = build_config(state_root);
    let runtime = SearchdRuntime::build(config)?;
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

    let req = SearchPlaneIpcRequestEnvelope {
        request_id: 0,
        payload: SearchPlaneIpcRequest::Semantic(SearchPlaneSemanticQueryRequest {
            query_text: "1.0".to_string(),
            generation: Some(GenerationPin::new(repo(), revision(), generation())),
            lexical_filters: LqFilterSet::default(),
            top_k: 3,
        }),
    };
    let response = send_request(&socket, &req)?;
    let err = match response.payload {
        SearchPlaneIpcResponse::Error(e) => e,
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

fn lex_query(needle: &str) -> SearchPlaneIpcRequestEnvelope {
    SearchPlaneIpcRequestEnvelope {
        request_id: 0,
        payload: SearchPlaneIpcRequest::Lexical(SearchPlaneLexicalQueryRequest {
            query: make_lq(LqExpr::Raw(needle.to_string())),
            generation: Some(GenerationPin::new(repo(), revision(), generation())),
        }),
    }
}
