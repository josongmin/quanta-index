//! Complex DSL scenario pack for the live repo-first search-plane.

#![forbid(unsafe_code)]
#![expect(
    clippy::let_underscore_untyped,
    reason = "publisher ops intentionally discard ack payloads in integration setup"
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

use anyhow::Result as AnyResult;
use quanta_index_channel::{
    BundleChannelPublisher, open_lexical_publisher, open_semantic_publisher,
};
use quanta_index_contract::{
    BridgeQueryRequest, BridgeScope, BridgeTarget, ChunkId, ChunkRecord, EmbeddingId,
    EngineTouched, GenerationPin, HybridQueryRequest, LexicalCandidate, LexicalChannelOp,
    LexicalFullBundle, LexicalRepoMetadataRecord, LqVisibility, ManifestGeneration, PlannerStage,
    RepoId, RepoRelativePath, RevisionId, SearchPlaneQueryIpcRequest,
    SearchPlaneQueryIpcRequestEnvelope, SearchPlaneQueryIpcResponse,
    SearchPlaneQueryIpcResponseEnvelope, SemanticCandidateScope, SemanticChannelOp,
    SemanticFullBundle, SemanticQueryRequest, TextQueryRequest, TextQuerySyntax, UpsertChunk,
    UpsertEmbedding,
};
use quanta_index_ipc::send_request;
use quanta_index_lq_bridge::TRANSLATOR_VERSION;
use quanta_index_searchd::app::SearchdConfig;
use quanta_index_searchd::app::searchd::drive;
use quanta_index_searchd_runtime::build_runtime;

type TestResult = Result<(), Box<dyn Error>>;
type DriverJoin = thread::JoinHandle<AnyResult<()>>;

static NEXT_SOCKET_ID: AtomicU64 = AtomicU64::new(0);
const READINESS_TIMEOUT: Duration = Duration::from_secs(15);

fn repo() -> RepoId {
    RepoId::new("repo-dsl")
}

fn revision() -> RevisionId {
    RevisionId::new("rev-dsl")
}

fn generation() -> ManifestGeneration {
    ManifestGeneration::new(21)
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
    let record = ChunkRecord {
        repo_relative_path: RepoRelativePath::new(repo_relative_path),
        language: language.to_string().into_boxed_str(),
        start_line,
        end_line,
        snippet: text.to_string().into_boxed_str(),
    };
    let mut buf = Vec::new();
    ciborium::into_writer(&record, &mut buf)
        .map_err(|err| -> Box<dyn Error> { format!("encode chunk: {err}").into() })?;
    Ok(buf)
}

fn repo_metadata_payload(
    fork: bool,
    archived: bool,
    visibility: LqVisibility,
    contexts: &[&str],
) -> Result<Vec<u8>, Box<dyn Error>> {
    let record = LexicalRepoMetadataRecord {
        fork,
        archived,
        visibility,
        contexts: contexts.iter().map(ToString::to_string).collect(),
    };
    let mut buf = Vec::new();
    ciborium::into_writer(&record, &mut buf)
        .map_err(|err| -> Box<dyn Error> { format!("encode repo metadata: {err}").into() })?;
    Ok(buf)
}

fn unique_socket_paths() -> (PathBuf, PathBuf) {
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let sequence = NEXT_SOCKET_ID.fetch_add(1, Ordering::Relaxed);
    let query = std::env::temp_dir().join(format!("qi-dsl-query-{pid}-{nanos}-{sequence}.sock"));
    let control =
        std::env::temp_dir().join(format!("qi-dsl-control-{pid}-{nanos}-{sequence}.sock"));
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

fn float_vec_to_bytes(vec: &[f32]) -> Result<Vec<u8>, Box<dyn Error>> {
    let owned: Vec<f32> = vec.to_vec();
    let mut out: Vec<u8> = Vec::new();
    ciborium::into_writer(&owned, &mut out)
        .map_err(|err| -> Box<dyn Error> { format!("ciborium encode embedding: {err}").into() })?;
    Ok(out)
}

fn float_vec_to_query_text(vec: &[f32]) -> String {
    vec.iter()
        .map(|value| format!("{value}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn start_runtime(
    state_root: &Path,
    thread_name: &str,
) -> Result<(PathBuf, Arc<AtomicBool>, DriverJoin), Box<dyn Error>> {
    let config = build_config(state_root);
    let runtime = build_runtime(config)?;
    let socket = runtime.query_server.socket_path().to_path_buf();
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_for_drive = Arc::clone(&shutdown);
    let join = thread::Builder::new()
        .name(thread_name.into())
        .spawn(move || drive(runtime, shutdown_for_drive))?;
    Ok((socket, shutdown, join))
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "callers consume the Arc<AtomicBool> at end-of-test; by-value avoids forcing & at every callsite"
)]
fn stop_runtime(shutdown: Arc<AtomicBool>, join: DriverJoin) -> TestResult {
    shutdown.store(true, Ordering::Release);
    match join.join() {
        Ok(Ok(())) => Ok(()),
        Ok(Err(err)) => Err(err.into()),
        Err(panic) => Err(format!("driver panic: {panic:?}").into()),
    }
}

fn pin() -> GenerationPin {
    GenerationPin::new(repo(), revision(), generation())
}

fn lexical_request(
    request_id: u64,
    syntax: TextQuerySyntax,
    query_text: &str,
) -> SearchPlaneQueryIpcRequestEnvelope {
    SearchPlaneQueryIpcRequestEnvelope {
        request_id,
        payload: SearchPlaneQueryIpcRequest::Text(TextQueryRequest {
            syntax,
            query_text: query_text.to_string(),
            generation: Some(pin()),
            generation_selector: None,
            top_k: 50,
        }),
    }
}

fn lexical_ids(results: &[LexicalCandidate]) -> Vec<String> {
    results
        .iter()
        .map(|candidate| candidate.candidate_id.clone())
        .collect()
}

fn sort_ids(mut ids: Vec<String>) -> Vec<String> {
    ids.sort();
    ids
}

fn wait_for_non_error(socket: &Path, request: &SearchPlaneQueryIpcRequestEnvelope) -> bool {
    wait_until(READINESS_TIMEOUT, || {
        match send_query_request(socket, request) {
            Ok(response) => !matches!(response.payload, SearchPlaneQueryIpcResponse::Error(_)),
            Err(_) => false,
        }
    })
}

#[test]
fn sourcegraph_repo_path_lang_filters_are_deterministic_across_repeated_runs() -> TestResult {
    let dir = tempfile::tempdir()?;
    let state_root = dir.path();

    {
        let publisher = open_lexical_publisher(state_root)?;
        let _ = publisher.publish(LexicalChannelOp::FullBundle(LexicalFullBundle {
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
        for (id, path, language, start_line, end_line, payload) in [
            (
                "alpha",
                "src/lib.rs",
                "rust",
                10_u32,
                14_u32,
                "needle rust alpha",
            ),
            (
                "beta",
                "src/main.rs",
                "rust",
                21_u32,
                26_u32,
                "needle rust beta",
            ),
            (
                "gamma",
                "src/lib.py",
                "python",
                30_u32,
                35_u32,
                "needle python gamma",
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

    let (socket, shutdown, join) = start_runtime(state_root, "dsl-sg-metadata-filters")?;
    if !wait_until(Duration::from_secs(2), || socket.exists()) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("socket never appeared".into());
    }

    let query_text = "repo:repo-dsl path:src/lib.rs lang:rust fork:no archived:no visibility:public context:global needle";
    let request = lexical_request(2, TextQuerySyntax::Sourcegraph, query_text);
    if !wait_for_non_error(&socket, &request) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("sourcegraph metadata query never became ready".into());
    }

    for _ in 0..5_u8 {
        let response = send_query_request(&socket, &request)?;
        let results = match response.payload {
            SearchPlaneQueryIpcResponse::Text(lexical) => lexical.results,
            other @ (SearchPlaneQueryIpcResponse::Symbol(_)
            | SearchPlaneQueryIpcResponse::Semantic(_)
            | SearchPlaneQueryIpcResponse::Hybrid(_)
            | SearchPlaneQueryIpcResponse::History(_)
            | SearchPlaneQueryIpcResponse::Structural(_)
            | SearchPlaneQueryIpcResponse::Bridge(_)
            | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
            | SearchPlaneQueryIpcResponse::Explain(_)
            | SearchPlaneQueryIpcResponse::Error(_)
            | SearchPlaneQueryIpcResponse::Sourcegraph(_)) => {
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
        if candidate.start_line != 10 || candidate.end_line != 14 {
            shutdown.store(true, Ordering::Release);
            drop(join.join());
            return Err(format!(
                "expected line span 10..14, got {}..{}",
                candidate.start_line, candidate.end_line
            )
            .into());
        }
    }

    let negative_request = lexical_request(3, TextQuerySyntax::Sourcegraph, "archived:only needle");
    let negative_response = send_query_request(&socket, &negative_request)?;
    let negative_results = match negative_response.payload {
        SearchPlaneQueryIpcResponse::Text(lexical) => lexical.results,
        other => {
            shutdown.store(true, Ordering::Release);
            drop(join.join());
            return Err(format!("expected Text for archived negative query, got {other:?}").into());
        }
    };
    if !negative_results.is_empty() {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!(
            "expected archived:only to return 0 hits for non-archived repo, got {:?}",
            lexical_ids(&negative_results)
        )
        .into());
    }

    stop_runtime(shutdown, join)
}

#[test]
fn sourcegraph_boolean_text_query_is_deterministic_across_repeated_runs() -> TestResult {
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
        for (id, payload) in [
            ("alpha", "sphinx alpha needle"),
            ("beta", "beta needle"),
            ("gamma", "sphinx gamma"),
            ("delta", "beta needle forbidden"),
        ] {
            let _ = publisher.publish(LexicalChannelOp::UpsertChunk(UpsertChunk {
                repo_id: repo(),
                revision_id: revision(),
                generation: generation(),
                chunk_id: ChunkId::new(id),
                payload: chunk_payload(payload)?,
            }))?;
        }
        let _ = publisher.seal(repo(), revision(), generation())?;
    }

    let (socket, shutdown, join) = start_runtime(state_root, "dsl-sg-determinism")?;
    if !wait_until(Duration::from_secs(2), || socket.exists()) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("socket never appeared".into());
    }

    let query_text = "(sphinx OR beta) needle NOT forbidden";
    let request = lexical_request(1, TextQuerySyntax::Sourcegraph, query_text);
    if !wait_for_non_error(&socket, &request) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("sourcegraph boolean query never became ready".into());
    }

    let mut baseline: Option<Vec<String>> = None;
    for _ in 0..10_u8 {
        let response = send_query_request(&socket, &request)?;
        let ids = match response.payload {
            SearchPlaneQueryIpcResponse::Text(lexical) => lexical_ids(&lexical.results),
            other @ (SearchPlaneQueryIpcResponse::Symbol(_)
            | SearchPlaneQueryIpcResponse::Semantic(_)
            | SearchPlaneQueryIpcResponse::Hybrid(_)
            | SearchPlaneQueryIpcResponse::History(_)
            | SearchPlaneQueryIpcResponse::Structural(_)
            | SearchPlaneQueryIpcResponse::Bridge(_)
            | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
            | SearchPlaneQueryIpcResponse::Explain(_)
            | SearchPlaneQueryIpcResponse::Error(_)
            | SearchPlaneQueryIpcResponse::Sourcegraph(_)) => {
                shutdown.store(true, Ordering::Release);
                drop(join.join());
                return Err(format!("expected Lexical, got {other:?}").into());
            }
        };
        if let Some(expected) = baseline.as_ref() {
            if &ids != expected {
                shutdown.store(true, Ordering::Release);
                drop(join.join());
                return Err(format!(
                    "lexical ordering drifted across repeated runs: expected {expected:?}, got {ids:?}"
                )
                .into());
            }
        } else {
            baseline = Some(ids);
        }
    }

    let observed = baseline.ok_or_else(|| "missing baseline lexical ids".to_string())?;
    if sort_ids(observed.clone()) != ["alpha".to_string(), "beta".to_string()] {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!("unexpected sourcegraph lexical ids: {observed:?}").into());
    }

    stop_runtime(shutdown, join)
}

#[test]
fn sourcegraph_repo_has_file_predicate_executes_live() -> TestResult {
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
        for (id, path, payload) in [
            ("alpha", "src/lib.rs", "needle alpha"),
            ("beta", "src/main.rs", "needle beta"),
            ("gamma", "docs/readme.md", "other text"),
        ] {
            let _ = publisher.publish(LexicalChannelOp::UpsertChunk(UpsertChunk {
                repo_id: repo(),
                revision_id: revision(),
                generation: generation(),
                chunk_id: ChunkId::new(id),
                payload: chunk_payload_with_metadata(path, "rust", 1, 2, payload)?,
            }))?;
        }
        let _ = publisher.seal(repo(), revision(), generation())?;
    }

    let (socket, shutdown, join) = start_runtime(state_root, "dsl-sg-repo-has-file")?;
    if !wait_until(Duration::from_secs(2), || socket.exists()) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("socket never appeared".into());
    }

    let request = lexical_request(
        13,
        TextQuerySyntax::Sourcegraph,
        "repo:has.file(path:src/lib.rs) needle",
    );
    if !wait_for_non_error(&socket, &request) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("repo.has.file query never became ready".into());
    }
    let ids = match send_query_request(&socket, &request)?.payload {
        SearchPlaneQueryIpcResponse::Text(lexical) => lexical_ids(&lexical.results),
        other @ (SearchPlaneQueryIpcResponse::Symbol(_)
        | SearchPlaneQueryIpcResponse::Semantic(_)
        | SearchPlaneQueryIpcResponse::Hybrid(_)
        | SearchPlaneQueryIpcResponse::History(_)
        | SearchPlaneQueryIpcResponse::Structural(_)
        | SearchPlaneQueryIpcResponse::Bridge(_)
        | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
        | SearchPlaneQueryIpcResponse::Explain(_)
        | SearchPlaneQueryIpcResponse::Error(_)
        | SearchPlaneQueryIpcResponse::Sourcegraph(_)) => {
            shutdown.store(true, Ordering::Release);
            drop(join.join());
            return Err(format!("expected Lexical, got {other:?}").into());
        }
    };
    if sort_ids(ids.clone()) != ["alpha".to_string(), "beta".to_string()] {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!("unexpected repo.has.file ids: {ids:?}").into());
    }

    let miss_request = lexical_request(
        14,
        TextQuerySyntax::Sourcegraph,
        "repo:has.file(path:missing.rs) needle",
    );
    if !wait_for_non_error(&socket, &miss_request) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("repo.has.file miss query never became ready".into());
    }
    let miss_ids = match send_query_request(&socket, &miss_request)?.payload {
        SearchPlaneQueryIpcResponse::Text(lexical) => lexical_ids(&lexical.results),
        other @ (SearchPlaneQueryIpcResponse::Symbol(_)
        | SearchPlaneQueryIpcResponse::Semantic(_)
        | SearchPlaneQueryIpcResponse::Hybrid(_)
        | SearchPlaneQueryIpcResponse::History(_)
        | SearchPlaneQueryIpcResponse::Structural(_)
        | SearchPlaneQueryIpcResponse::Bridge(_)
        | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
        | SearchPlaneQueryIpcResponse::Explain(_)
        | SearchPlaneQueryIpcResponse::Error(_)
        | SearchPlaneQueryIpcResponse::Sourcegraph(_)) => {
            shutdown.store(true, Ordering::Release);
            drop(join.join());
            return Err(format!("expected Lexical for miss case, got {other:?}").into());
        }
    };
    if !miss_ids.is_empty() {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!("expected 0 repo.has.file miss ids, got {miss_ids:?}").into());
    }

    stop_runtime(shutdown, join)
}

#[test]
fn sourcegraph_phrase_and_regex_patterns_execute_live() -> TestResult {
    let dir = tempfile::tempdir()?;
    let state_root = dir.path();

    {
        let publisher = open_lexical_publisher(state_root)?;
        for (id, payload) in [
            ("alpha", "sphinx of quartz"),
            ("beta", "riddle42"),
            ("gamma", "sphinx of clay"),
        ] {
            let _ = publisher.publish(LexicalChannelOp::UpsertChunk(UpsertChunk {
                repo_id: repo(),
                revision_id: revision(),
                generation: generation(),
                chunk_id: ChunkId::new(id),
                payload: chunk_payload(payload)?,
            }))?;
        }
        let _ = publisher.seal(repo(), revision(), generation())?;
    }

    let (socket, shutdown, join) = start_runtime(state_root, "dsl-sg-phrase-regex")?;
    if !wait_until(Duration::from_secs(2), || socket.exists()) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("socket never appeared".into());
    }

    let request = lexical_request(
        11,
        TextQuerySyntax::Sourcegraph,
        "\"sphinx of quartz\" OR /riddle[0-9]+/",
    );
    if !wait_until(READINESS_TIMEOUT, || {
        match send_query_request(&socket, &request) {
            Ok(response) => match response.payload {
                SearchPlaneQueryIpcResponse::Text(_)
                | SearchPlaneQueryIpcResponse::Symbol(_)
                | SearchPlaneQueryIpcResponse::Semantic(_)
                | SearchPlaneQueryIpcResponse::Hybrid(_)
                | SearchPlaneQueryIpcResponse::History(_)
                | SearchPlaneQueryIpcResponse::Structural(_)
                | SearchPlaneQueryIpcResponse::Bridge(_)
                | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
                | SearchPlaneQueryIpcResponse::Explain(_)
                | SearchPlaneQueryIpcResponse::Sourcegraph(_) => true,
                SearchPlaneQueryIpcResponse::Error(err) => err.code != "NOT_READY",
            },
            Err(_) => false,
        }
    }) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("sourcegraph phrase/regex query never progressed past NOT_READY".into());
    }

    let response = send_query_request(&socket, &request)?;
    let ids = match response.payload {
        SearchPlaneQueryIpcResponse::Text(lexical) => lexical_ids(&lexical.results),
        other @ (SearchPlaneQueryIpcResponse::Symbol(_)
        | SearchPlaneQueryIpcResponse::Semantic(_)
        | SearchPlaneQueryIpcResponse::Hybrid(_)
        | SearchPlaneQueryIpcResponse::History(_)
        | SearchPlaneQueryIpcResponse::Structural(_)
        | SearchPlaneQueryIpcResponse::Bridge(_)
        | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
        | SearchPlaneQueryIpcResponse::Explain(_)
        | SearchPlaneQueryIpcResponse::Error(_)
        | SearchPlaneQueryIpcResponse::Sourcegraph(_)) => {
            shutdown.store(true, Ordering::Release);
            drop(join.join());
            return Err(format!("expected Lexical, got {other:?}").into());
        }
    };
    if sort_ids(ids.clone()) != ["alpha".to_string(), "beta".to_string()] {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!("unexpected Sourcegraph phrase/regex ids: {ids:?}").into());
    }

    let regexp_option_request = lexical_request(
        12,
        TextQuerySyntax::Sourcegraph,
        "patterntype:regexp riddle[0-9]+",
    );
    if !wait_for_non_error(&socket, &regexp_option_request) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("sourcegraph patterntype:regexp query never became ready".into());
    }
    let regexp_option_ids = match send_query_request(&socket, &regexp_option_request)?.payload {
        SearchPlaneQueryIpcResponse::Text(lexical) => lexical_ids(&lexical.results),
        other @ (SearchPlaneQueryIpcResponse::Symbol(_)
        | SearchPlaneQueryIpcResponse::Semantic(_)
        | SearchPlaneQueryIpcResponse::Hybrid(_)
        | SearchPlaneQueryIpcResponse::History(_)
        | SearchPlaneQueryIpcResponse::Structural(_)
        | SearchPlaneQueryIpcResponse::Bridge(_)
        | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
        | SearchPlaneQueryIpcResponse::Explain(_)
        | SearchPlaneQueryIpcResponse::Error(_)
        | SearchPlaneQueryIpcResponse::Sourcegraph(_)) => {
            shutdown.store(true, Ordering::Release);
            drop(join.join());
            return Err(format!("expected Lexical for patterntype:regexp, got {other:?}").into());
        }
    };
    if regexp_option_ids != ["beta".to_string()] {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!(
            "unexpected Sourcegraph patterntype:regexp ids: {regexp_option_ids:?}"
        )
        .into());
    }

    stop_runtime(shutdown, join)
}

#[test]
fn lq_phrase_and_regex_patterns_execute_live() -> TestResult {
    let dir = tempfile::tempdir()?;
    let state_root = dir.path();

    {
        let publisher = open_lexical_publisher(state_root)?;
        for (id, payload) in [
            ("alpha", "sphinx of quartz"),
            ("beta", "riddle42"),
            ("gamma", "sphinx of clay"),
        ] {
            let _ = publisher.publish(LexicalChannelOp::UpsertChunk(UpsertChunk {
                repo_id: repo(),
                revision_id: revision(),
                generation: generation(),
                chunk_id: ChunkId::new(id),
                payload: chunk_payload(payload)?,
            }))?;
        }
        let _ = publisher.seal(repo(), revision(), generation())?;
    }

    let (socket, shutdown, join) = start_runtime(state_root, "dsl-lq-phrase-regex")?;
    if !wait_until(Duration::from_secs(2), || socket.exists()) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("socket never appeared".into());
    }

    let phrase_request = lexical_request(
        2,
        TextQuerySyntax::Native,
        "\"sphinx of quartz\" OR riddle42",
    );
    if !wait_for_non_error(&socket, &phrase_request) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("LQ phrase query never became ready".into());
    }

    let response = send_query_request(&socket, &phrase_request)?;
    let ids = match response.payload {
        SearchPlaneQueryIpcResponse::Text(lexical) => lexical_ids(&lexical.results),
        other @ (SearchPlaneQueryIpcResponse::Symbol(_)
        | SearchPlaneQueryIpcResponse::Semantic(_)
        | SearchPlaneQueryIpcResponse::Hybrid(_)
        | SearchPlaneQueryIpcResponse::History(_)
        | SearchPlaneQueryIpcResponse::Structural(_)
        | SearchPlaneQueryIpcResponse::Bridge(_)
        | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
        | SearchPlaneQueryIpcResponse::Explain(_)
        | SearchPlaneQueryIpcResponse::Error(_)
        | SearchPlaneQueryIpcResponse::Sourcegraph(_)) => {
            shutdown.store(true, Ordering::Release);
            drop(join.join());
            return Err(format!("expected Lexical, got {other:?}").into());
        }
    };
    if sort_ids(ids.clone()) != ["alpha".to_string(), "beta".to_string()] {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!("unexpected LQ phrase ids: {ids:?}").into());
    }

    let regex_request = lexical_request(3, TextQuerySyntax::Native, "/riddle[0-9]+/");
    if !wait_for_non_error(&socket, &regex_request) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("LQ regex query never became ready".into());
    }
    let regex_ids = match send_query_request(&socket, &regex_request)?.payload {
        SearchPlaneQueryIpcResponse::Text(lexical) => lexical_ids(&lexical.results),
        other @ (SearchPlaneQueryIpcResponse::Symbol(_)
        | SearchPlaneQueryIpcResponse::Semantic(_)
        | SearchPlaneQueryIpcResponse::Hybrid(_)
        | SearchPlaneQueryIpcResponse::History(_)
        | SearchPlaneQueryIpcResponse::Structural(_)
        | SearchPlaneQueryIpcResponse::Bridge(_)
        | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
        | SearchPlaneQueryIpcResponse::Explain(_)
        | SearchPlaneQueryIpcResponse::Error(_)
        | SearchPlaneQueryIpcResponse::Sourcegraph(_)) => {
            shutdown.store(true, Ordering::Release);
            drop(join.join());
            return Err(format!("expected live regex result, got {other:?}").into());
        }
    };
    if regex_ids != ["beta".to_string()] {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!("unexpected LQ regex ids: {regex_ids:?}").into());
    }

    stop_runtime(shutdown, join)
}

#[test]
fn semantic_scoped_query_with_complex_scope_excludes_outsiders_and_explains_scope() -> TestResult {
    let dir = tempfile::tempdir()?;
    let state_root = dir.path();

    {
        let publisher = open_lexical_publisher(state_root)?;
        for (id, payload) in [
            ("alpha", "scope alpha keep"),
            ("beta", "scope beta keep"),
            ("gamma", "scope alpha outsider"),
            ("omega", "global outsider"),
        ] {
            let _ = publisher.publish(LexicalChannelOp::UpsertChunk(UpsertChunk {
                repo_id: repo(),
                revision_id: revision(),
                generation: generation(),
                chunk_id: ChunkId::new(id),
                payload: chunk_payload(payload)?,
            }))?;
        }
        let _ = publisher.seal(repo(), revision(), generation())?;
    }
    {
        let publisher = open_semantic_publisher(state_root)?;
        let _ = publisher.publish(SemanticChannelOp::FullBundle(SemanticFullBundle {
            repo_id: repo(),
            revision_id: revision(),
            generation: generation(),
            payload: Vec::new(),
        }))?;
        for (id, vector) in [
            ("alpha", vec![0.9_f32, 0.1_f32]),
            ("beta", vec![0.8_f32, 0.2_f32]),
            ("gamma", vec![0.99_f32, 0.01_f32]),
            ("omega", vec![1.0_f32, 0.0_f32]),
        ] {
            let _ = publisher.publish(SemanticChannelOp::UpsertEmbedding(UpsertEmbedding {
                repo_id: repo(),
                revision_id: revision(),
                generation: generation(),
                embedding_id: EmbeddingId::new(id),
                payload: float_vec_to_bytes(&vector)?,
            }))?;
        }
        let _ = publisher.seal(repo(), revision(), generation())?;
    }

    let (socket, shutdown, join) = start_runtime(state_root, "dsl-semantic-complex-scope")?;
    if !wait_until(Duration::from_secs(2), || socket.exists()) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("socket never appeared".into());
    }

    let request = SearchPlaneQueryIpcRequestEnvelope {
        request_id: 3,
        payload: SearchPlaneQueryIpcRequest::Semantic(SemanticQueryRequest {
            query_text: Some(float_vec_to_query_text(&[1.0_f32, 0.0_f32])),
            query_vector: None,
            query_vector_ref: None,
            generation: Some(pin()),
            generation_selector: None,
            scope: Some(SemanticCandidateScope {
                syntax: TextQuerySyntax::Sourcegraph,
                query_text: "(alpha OR beta) scope NOT outsider".to_string(),
                generation: Some(pin()),
                generation_selector: None,
            }),
            top_k: 2,
        }),
    };
    if !wait_for_non_error(&socket, &request) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("semantic scoped complex query never became ready".into());
    }

    let response = send_query_request(&socket, &request)?;
    let (ids, explanation) = match response.payload {
        SearchPlaneQueryIpcResponse::Semantic(semantic) => {
            (lexical_ids(&semantic.results), semantic.explanation)
        }
        other @ (SearchPlaneQueryIpcResponse::Text(_)
        | SearchPlaneQueryIpcResponse::Symbol(_)
        | SearchPlaneQueryIpcResponse::Hybrid(_)
        | SearchPlaneQueryIpcResponse::History(_)
        | SearchPlaneQueryIpcResponse::Structural(_)
        | SearchPlaneQueryIpcResponse::Bridge(_)
        | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
        | SearchPlaneQueryIpcResponse::Explain(_)
        | SearchPlaneQueryIpcResponse::Error(_)
        | SearchPlaneQueryIpcResponse::Sourcegraph(_)) => {
            shutdown.store(true, Ordering::Release);
            drop(join.join());
            return Err(format!("expected Semantic, got {other:?}").into());
        }
    };
    if sort_ids(ids.clone()) != ["alpha".to_string(), "beta".to_string()] {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!("unexpected semantic scoped ids: {ids:?}").into());
    }
    if explanation.strategy != "semantic_scoped" {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!(
            "unexpected semantic explanation strategy: {}",
            explanation.strategy
        )
        .into());
    }
    if explanation.engines_touched != vec![EngineTouched::Lexical, EngineTouched::Semantic] {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!(
            "unexpected semantic engines_touched: {:?}",
            explanation.engines_touched
        )
        .into());
    }
    if !explanation.summary.contains("text scope of 2") {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!(
            "semantic explanation summary missing scope accounting: {}",
            explanation.summary
        )
        .into());
    }
    let has_scope_plan = explanation
        .planner_trace
        .iter()
        .any(|entry| entry.stage == PlannerStage::Plan && entry.detail == "semantic.scope=true");
    let has_scope_exec = explanation.planner_trace.iter().any(|entry| {
        entry.stage == PlannerStage::ExecFanout
            && entry.detail == "semantic.scope.text_candidates=2"
    });
    if !has_scope_plan || !has_scope_exec {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!(
            "semantic planner trace missing scoped details: {:?}",
            explanation.planner_trace
        )
        .into());
    }

    stop_runtime(shutdown, join)
}

#[test]
fn hybrid_query_reports_complex_scope_explanation_accounting() -> TestResult {
    let dir = tempfile::tempdir()?;
    let state_root = dir.path();

    {
        let publisher = open_lexical_publisher(state_root)?;
        for (id, payload) in [
            ("alpha", "scope alpha keep"),
            ("beta", "scope beta keep"),
            ("gamma", "scope alpha outsider"),
            ("omega", "global outsider"),
        ] {
            let _ = publisher.publish(LexicalChannelOp::UpsertChunk(UpsertChunk {
                repo_id: repo(),
                revision_id: revision(),
                generation: generation(),
                chunk_id: ChunkId::new(id),
                payload: chunk_payload(payload)?,
            }))?;
        }
        let _ = publisher.seal(repo(), revision(), generation())?;
    }
    {
        let publisher = open_semantic_publisher(state_root)?;
        let _ = publisher.publish(SemanticChannelOp::FullBundle(SemanticFullBundle {
            repo_id: repo(),
            revision_id: revision(),
            generation: generation(),
            payload: Vec::new(),
        }))?;
        for (id, vector) in [
            ("alpha", vec![0.9_f32, 0.1_f32]),
            ("beta", vec![0.8_f32, 0.2_f32]),
            ("gamma", vec![0.99_f32, 0.01_f32]),
            ("omega", vec![1.0_f32, 0.0_f32]),
        ] {
            let _ = publisher.publish(SemanticChannelOp::UpsertEmbedding(UpsertEmbedding {
                repo_id: repo(),
                revision_id: revision(),
                generation: generation(),
                embedding_id: EmbeddingId::new(id),
                payload: float_vec_to_bytes(&vector)?,
            }))?;
        }
        let _ = publisher.seal(repo(), revision(), generation())?;
    }

    let (socket, shutdown, join) = start_runtime(state_root, "dsl-hybrid-complex-scope")?;
    if !wait_until(Duration::from_secs(2), || socket.exists()) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("socket never appeared".into());
    }

    let request = SearchPlaneQueryIpcRequestEnvelope {
        request_id: 4,
        payload: SearchPlaneQueryIpcRequest::Hybrid(HybridQueryRequest {
            text_query: TextQueryRequest {
                syntax: TextQuerySyntax::Sourcegraph,
                query_text: "(alpha OR beta) scope NOT outsider".to_string(),
                generation: Some(pin()),
                generation_selector: None,
                top_k: 50,
            },
            semantic_query_text: Some(float_vec_to_query_text(&[1.0_f32, 0.0_f32])),
            semantic_vector: None,
            semantic_vector_ref: None,
            generation: Some(pin()),
            generation_selector: None,
            top_k: 2,
        }),
    };
    if !wait_for_non_error(&socket, &request) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("hybrid complex query never became ready".into());
    }

    let response = send_query_request(&socket, &request)?;
    let (ids, explanation) = match response.payload {
        SearchPlaneQueryIpcResponse::Hybrid(hybrid) => {
            (lexical_ids(&hybrid.results), hybrid.explanation)
        }
        other @ (SearchPlaneQueryIpcResponse::Text(_)
        | SearchPlaneQueryIpcResponse::Symbol(_)
        | SearchPlaneQueryIpcResponse::Semantic(_)
        | SearchPlaneQueryIpcResponse::History(_)
        | SearchPlaneQueryIpcResponse::Structural(_)
        | SearchPlaneQueryIpcResponse::Bridge(_)
        | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
        | SearchPlaneQueryIpcResponse::Explain(_)
        | SearchPlaneQueryIpcResponse::Error(_)
        | SearchPlaneQueryIpcResponse::Sourcegraph(_)) => {
            shutdown.store(true, Ordering::Release);
            drop(join.join());
            return Err(format!("expected Hybrid, got {other:?}").into());
        }
    };
    if sort_ids(ids.clone()) != ["alpha".to_string(), "beta".to_string()] {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!("unexpected hybrid ids: {ids:?}").into());
    }
    if explanation.strategy != "rrf" {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!(
            "unexpected hybrid explanation strategy: {}",
            explanation.strategy
        )
        .into());
    }
    if explanation.engines_touched != vec![EngineTouched::Lexical, EngineTouched::Semantic] {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!(
            "unexpected hybrid engines_touched: {:?}",
            explanation.engines_touched
        )
        .into());
    }
    let has_plan = explanation.planner_trace.iter().any(|entry| {
        entry.stage == PlannerStage::Plan && entry.detail == "hybrid.internal_top_k=100"
    });
    let has_exec = explanation.planner_trace.iter().any(|entry| {
        entry.stage == PlannerStage::ExecFanout
            && entry.detail == "hybrid.lexical_universe=2; lexical_hits=2; semantic_hits=2"
    });
    let has_merge = explanation.planner_trace.iter().any(|entry| {
        entry.stage == PlannerStage::Merge && entry.detail == "hybrid.fused_results=2"
    });
    if !has_plan || !has_exec || !has_merge {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!(
            "hybrid planner trace missing accounting details: {:?}",
            explanation.planner_trace
        )
        .into());
    }
    if explanation.summary != "hybrid fused 2 lexical and 2 semantic candidates into 2 results" {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!(
            "unexpected hybrid explanation summary: {}",
            explanation.summary
        )
        .into());
    }

    stop_runtime(shutdown, join)
}

#[test]
fn bridge_query_preserves_complex_sourcegraph_metadata_and_candidate_set() -> TestResult {
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
        for (id, payload) in [
            ("alpha", "sphinx alpha needle"),
            ("beta", "beta needle"),
            ("gamma", "sphinx gamma"),
            ("delta", "beta needle forbidden"),
        ] {
            let _ = publisher.publish(LexicalChannelOp::UpsertChunk(UpsertChunk {
                repo_id: repo(),
                revision_id: revision(),
                generation: generation(),
                chunk_id: ChunkId::new(id),
                payload: chunk_payload(payload)?,
            }))?;
        }
        let _ = publisher.seal(repo(), revision(), generation())?;
    }

    let (socket, shutdown, join) = start_runtime(state_root, "dsl-bridge-metadata")?;
    if !wait_until(Duration::from_secs(2), || socket.exists()) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("socket never appeared".into());
    }

    let query_text = "(sphinx OR beta) needle NOT forbidden";
    let lexical_probe = lexical_request(5, TextQuerySyntax::Sourcegraph, query_text);
    if !wait_for_non_error(&socket, &lexical_probe) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("bridge lexical complex query never became ready".into());
    }

    let response = send_query_request(
        &socket,
        &SearchPlaneQueryIpcRequestEnvelope {
            request_id: 6,
            payload: SearchPlaneQueryIpcRequest::Bridge(BridgeQueryRequest {
                text_query: TextQueryRequest {
                    syntax: TextQuerySyntax::Sourcegraph,
                    query_text: query_text.to_string(),
                    generation: Some(pin()),
                    generation_selector: None,
                    top_k: 50,
                },
                target: BridgeTarget::CodeQl,
            }),
        },
    )?;
    let bridge = match response.payload {
        SearchPlaneQueryIpcResponse::Bridge(bridge) => bridge,
        other @ (SearchPlaneQueryIpcResponse::Text(_)
        | SearchPlaneQueryIpcResponse::Symbol(_)
        | SearchPlaneQueryIpcResponse::Semantic(_)
        | SearchPlaneQueryIpcResponse::Hybrid(_)
        | SearchPlaneQueryIpcResponse::History(_)
        | SearchPlaneQueryIpcResponse::Structural(_)
        | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
        | SearchPlaneQueryIpcResponse::Explain(_)
        | SearchPlaneQueryIpcResponse::Error(_)
        | SearchPlaneQueryIpcResponse::Sourcegraph(_)) => {
            shutdown.store(true, Ordering::Release);
            drop(join.join());
            return Err(format!("expected Bridge, got {other:?}").into());
        }
    };
    if bridge.packet.scope != BridgeScope::Lexical {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!("unexpected bridge scope: {:?}", bridge.packet.scope).into());
    }
    if bridge.packet.source_syntax.as_deref() != Some(query_text) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!(
            "unexpected bridge source_syntax: {:?}",
            bridge.packet.source_syntax
        )
        .into());
    }
    if bridge.packet.translator_version.as_deref() != Some(TRANSLATOR_VERSION) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!(
            "unexpected bridge translator_version: {:?}",
            bridge.packet.translator_version
        )
        .into());
    }
    let ids = lexical_ids(&bridge.packet.candidates);
    if sort_ids(ids.clone()) != ["alpha".to_string(), "beta".to_string()] {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!("unexpected bridge candidate ids: {ids:?}").into());
    }

    stop_runtime(shutdown, join)
}
