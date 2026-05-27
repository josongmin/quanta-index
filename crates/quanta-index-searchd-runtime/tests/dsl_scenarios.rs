//! Complex DSL scenario pack for the live repo-first search-plane.

#![forbid(unsafe_code)]
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
use quanta_index_contract::lex::LanguageCode;
use quanta_index_contract::{
    BatchIngestMode, ChunkId, ChunkRecord, EarlyStopReason, EngineTouched, GenerationPin,
    HybridQueryRequest,
    LexicalCandidate, LexicalIngestBatch, LexicalReplaceScope, LqVisibility, ManifestGeneration,
    PlannerStage, RepoId, RepoRelativePath, RevisionId, SearchPlaneIngestIpcRequest,
    SearchPlaneIngestIpcRequestEnvelope, SearchPlaneIngestIpcResponse,
    SearchPlaneIngestIpcResponseEnvelope, SearchPlaneQueryIpcRequest,
    SearchPlaneQueryIpcRequestEnvelope, SearchPlaneQueryIpcResponse,
    SearchPlaneQueryIpcResponseEnvelope, SearchScopeKey, SearchScopeSurface, SemanticQueryRequest,
    TextQueryRequest, TextQuerySyntax,
};
use quanta_index_ipc::send_request;
use quanta_index_searchd::app::SearchdConfig;
use quanta_index_searchd::app::searchd::drive;
use quanta_index_searchd_runtime::build_runtime;
use serde::ser::{Serialize, SerializeStruct, Serializer};
use std::collections::BTreeMap;

type TestResult = Result<(), Box<dyn Error>>;
type DriverJoin = thread::JoinHandle<AnyResult<()>>;
type RuntimeHandles = (PathBuf, PathBuf, Arc<AtomicBool>, DriverJoin);

static NEXT_SOCKET_ID: AtomicU64 = AtomicU64::new(0);
const READINESS_TIMEOUT: Duration = Duration::from_secs(15);
const SOCKET_APPEAR_TIMEOUT: Duration = Duration::from_secs(5);

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
    RepoId::new("repo-dsl")
}

fn revision() -> RevisionId {
    RevisionId::new("rev-dsl")
}

fn generation() -> ManifestGeneration {
    ManifestGeneration::new(21)
}

fn chunk_record(id: &str, text: &str) -> Result<ChunkRecord, Box<dyn Error>> {
    chunk_record_with_metadata(id, "src/dsl.txt", "text", 0, 0, text)
}

fn chunk_record_with_metadata(
    id: &str,
    repo_relative_path: &str,
    language: &str,
    start_line: u32,
    end_line: u32,
    text: &str,
) -> Result<ChunkRecord, Box<dyn Error>> {
    Ok(ChunkRecord {
        chunk_id: ChunkId::new(id),
        repo_relative_path: RepoRelativePath::new(repo_relative_path),
        language: LanguageCode::new(language)
            .map_err(|err| -> Box<dyn Error> { format!("invalid language code: {err}").into() })?,
        start_byte: 0,
        end_byte: u32::try_from(text.len()).map_err(|err| -> Box<dyn Error> {
            format!("chunk text length overflow: {err}").into()
        })?,
        start_line,
        end_line,
        text: text.to_string().into_boxed_str(),
        structural: None,
        parent_chunk_id: None,
    })
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

fn unique_socket_paths() -> (PathBuf, PathBuf, PathBuf) {
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let sequence = NEXT_SOCKET_ID.fetch_add(1, Ordering::Relaxed);
    let query = std::env::temp_dir().join(format!("qi-dsl-query-{pid}-{nanos}-{sequence}.sock"));
    let control =
        std::env::temp_dir().join(format!("qi-dsl-control-{pid}-{nanos}-{sequence}.sock"));
    let ingest = std::env::temp_dir().join(format!("qi-dsl-ingest-{pid}-{nanos}-{sequence}.sock"));
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

fn scope_key(path: &str) -> SearchScopeKey {
    SearchScopeKey {
        doc_surface: SearchScopeSurface::Chunk,
        repo_relative_path: RepoRelativePath::new(path),
    }
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
        SearchPlaneIngestIpcResponse::LexicalReceipt(_)
        | SearchPlaneIngestIpcResponse::HistoryReceipt(_)
        | SearchPlaneIngestIpcResponse::DirtyReceipt(_)
        | SearchPlaneIngestIpcResponse::StructuralReceipt(_)
        | SearchPlaneIngestIpcResponse::RepoMapReceipt(_) => Ok(()),
        SearchPlaneIngestIpcResponse::Error(err) => {
            Err(format!("ingest failed code={} message={}", err.code, err.message).into())
        }
    }
}

fn publish_lexical_chunks(
    socket: &Path,
    chunks: Vec<ChunkRecord>,
    bundle_payload: Option<Vec<u8>>,
) -> TestResult {
    let mut chunks_by_path: BTreeMap<String, Vec<ChunkRecord>> = BTreeMap::new();
    for chunk in chunks {
        chunks_by_path
            .entry(chunk.repo_relative_path.as_str().to_string())
            .or_default()
            .push(chunk);
    }
    let replace_scopes = chunks_by_path
        .into_iter()
        .map(|(path, chunks)| LexicalReplaceScope {
            scope: scope_key(&path),
            scope_digest: format!("dsl-lex-scope:{path}"),
            chunks,
            symbols: Vec::new(),
        })
        .collect();
    dispatch_ingest(
        socket,
        SearchPlaneIngestIpcRequest::PublishLexicalBatch(LexicalIngestBatch {
            repo_id: repo(),
            revision_id: revision(),
            generation: generation(),
            base_generation: None,
            manifest_digest: format!("dsl-lex-manifest-{}", generation().get()),
            batch_digest: format!(
                "dsl-lex-batch-{}",
                NEXT_SOCKET_ID.fetch_add(1, Ordering::Relaxed)
            ),
            mode: BatchIngestMode::Delta,
            bundle_payload,
            replace_scopes,
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
            manifest_digest: format!("dsl-lex-seal-{}", generation().get()),
            batch_digest: format!(
                "dsl-lex-seal-batch-{}",
                NEXT_SOCKET_ID.fetch_add(1, Ordering::Relaxed)
            ),
            mode: BatchIngestMode::Delta,
            bundle_payload: None,
            replace_scopes: Vec::new(),
            tombstone_scopes: Vec::new(),
            seal: true,
        }),
    )
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
    let (socket, ingest_socket, shutdown, join) =
        start_runtime(state_root, "dsl-sg-metadata-filters")?;
    publish_lexical_chunks(
        &ingest_socket,
        vec![
            chunk_record_with_metadata("alpha", "src/lib.rs", "rust", 10, 14, "needle rust alpha")?,
            chunk_record_with_metadata("beta", "src/main.rs", "rust", 21, 26, "needle rust beta")?,
            chunk_record_with_metadata(
                "gamma",
                "src/lib.py",
                "python",
                30,
                35,
                "needle python gamma",
            )?,
        ],
        Some(repo_metadata_payload(
            false,
            false,
            LqVisibility::Public,
            &["global", "team-search"],
        )?),
    )?;
    seal_lexical(&ingest_socket)?;

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
            | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
            | SearchPlaneQueryIpcResponse::Explain(_)
            | SearchPlaneQueryIpcResponse::Error(_)
            | quanta_index_contract::SearchPlaneQueryIpcResponse::RuntimeMetadata(_)) => {
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
    let (socket, ingest_socket, shutdown, join) = start_runtime(state_root, "dsl-sg-determinism")?;
    publish_lexical_chunks(
        &ingest_socket,
        vec![
            chunk_record("alpha", "sphinx alpha needle")?,
            chunk_record("beta", "beta needle")?,
            chunk_record("gamma", "sphinx gamma")?,
            chunk_record("delta", "beta needle forbidden")?,
        ],
        Some(b"manifest".to_vec()),
    )?;
    seal_lexical(&ingest_socket)?;

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
            | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
            | SearchPlaneQueryIpcResponse::Explain(_)
            | SearchPlaneQueryIpcResponse::Error(_)
            | quanta_index_contract::SearchPlaneQueryIpcResponse::RuntimeMetadata(_)) => {
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
    let (socket, ingest_socket, shutdown, join) =
        start_runtime(state_root, "dsl-sg-repo-has-file")?;
    publish_lexical_chunks(
        &ingest_socket,
        vec![
            chunk_record_with_metadata("alpha", "src/lib.rs", "rust", 1, 2, "needle alpha")?,
            chunk_record_with_metadata("beta", "src/main.rs", "rust", 1, 2, "needle beta")?,
            chunk_record_with_metadata("gamma", "docs/readme.md", "rust", 1, 2, "other text")?,
        ],
        Some(b"manifest".to_vec()),
    )?;
    seal_lexical(&ingest_socket)?;

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
        | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
        | SearchPlaneQueryIpcResponse::Explain(_)
        | SearchPlaneQueryIpcResponse::Error(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::RuntimeMetadata(_)) => {
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
        | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
        | SearchPlaneQueryIpcResponse::Explain(_)
        | SearchPlaneQueryIpcResponse::Error(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::RuntimeMetadata(_)) => {
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
    let (socket, ingest_socket, shutdown, join) = start_runtime(state_root, "dsl-sg-phrase-regex")?;
    publish_lexical_chunks(
        &ingest_socket,
        vec![
            chunk_record("alpha", "sphinx of quartz")?,
            chunk_record("beta", "riddle42")?,
            chunk_record("gamma", "sphinx of clay")?,
        ],
        None,
    )?;
    seal_lexical(&ingest_socket)?;

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
                | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
                | SearchPlaneQueryIpcResponse::Explain(_)
                | quanta_index_contract::SearchPlaneQueryIpcResponse::RuntimeMetadata(_) => true,
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
        | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
        | SearchPlaneQueryIpcResponse::Explain(_)
        | SearchPlaneQueryIpcResponse::Error(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::RuntimeMetadata(_)) => {
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
        | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
        | SearchPlaneQueryIpcResponse::Explain(_)
        | SearchPlaneQueryIpcResponse::Error(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::RuntimeMetadata(_)) => {
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
    let (socket, ingest_socket, shutdown, join) = start_runtime(state_root, "dsl-lq-phrase-regex")?;
    publish_lexical_chunks(
        &ingest_socket,
        vec![
            chunk_record("alpha", "sphinx of quartz")?,
            chunk_record("beta", "riddle42")?,
            chunk_record("gamma", "sphinx of clay")?,
        ],
        None,
    )?;
    seal_lexical(&ingest_socket)?;

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
        | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
        | SearchPlaneQueryIpcResponse::Explain(_)
        | SearchPlaneQueryIpcResponse::Error(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::RuntimeMetadata(_)) => {
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
        | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
        | SearchPlaneQueryIpcResponse::Explain(_)
        | SearchPlaneQueryIpcResponse::Error(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::RuntimeMetadata(_)) => {
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
    let (socket, ingest_socket, shutdown, join) =
        start_runtime(state_root, "dsl-semantic-complex-scope")?;
    publish_lexical_chunks(
        &ingest_socket,
        vec![
            chunk_record("alpha", "scope alpha keep")?,
            chunk_record("beta", "scope beta keep")?,
            chunk_record("gamma", "scope alpha outsider")?,
            chunk_record("omega", "global outsider")?,
        ],
        None,
    )?;
    seal_lexical(&ingest_socket)?;

    let request = SearchPlaneQueryIpcRequestEnvelope {
        request_id: 3,
        payload: SearchPlaneQueryIpcRequest::Semantic(SemanticQueryRequest {
            query_text: "scope".to_string(),
            generation: Some(pin()),
            generation_selector: None,
            lexical_scope: Some(TextQueryRequest {
                syntax: TextQuerySyntax::Sourcegraph,
                query_text: "(alpha OR beta) scope NOT outsider".to_string(),
                generation: Some(pin()),
                generation_selector: None,
                top_k: 2,
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
        | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
        | SearchPlaneQueryIpcResponse::Explain(_)
        | SearchPlaneQueryIpcResponse::Error(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::RuntimeMetadata(_)) => {
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
    let (socket, ingest_socket, shutdown, join) =
        start_runtime(state_root, "dsl-hybrid-complex-scope")?;
    publish_lexical_chunks(
        &ingest_socket,
        vec![
            chunk_record("alpha", "scope alpha keep")?,
            chunk_record("beta", "scope beta keep")?,
            chunk_record("gamma", "scope alpha outsider")?,
            chunk_record("omega", "global outsider")?,
        ],
        None,
    )?;
    seal_lexical(&ingest_socket)?;

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
            semantic_query_text: "scope".to_string(),
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
        | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
        | SearchPlaneQueryIpcResponse::Explain(_)
        | SearchPlaneQueryIpcResponse::Error(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::RuntimeMetadata(_)) => {
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
fn hybrid_query_surfaces_truthful_count_reached_early_stop() -> TestResult {
    let dir = tempfile::tempdir()?;
    let state_root = dir.path();
    let (socket, ingest_socket, shutdown, join) =
        start_runtime(state_root, "dsl-hybrid-count-reached")?;
    publish_lexical_chunks(
        &ingest_socket,
        vec![
            chunk_record("alpha", "scope alpha keep")?,
            chunk_record("beta", "scope beta keep")?,
            chunk_record("gamma", "scope gamma keep")?,
        ],
        None,
    )?;
    seal_lexical(&ingest_socket)?;

    let request = SearchPlaneQueryIpcRequestEnvelope {
        request_id: 5,
        payload: SearchPlaneQueryIpcRequest::Hybrid(HybridQueryRequest {
            text_query: TextQueryRequest {
                syntax: TextQuerySyntax::Sourcegraph,
                query_text: "scope".to_string(),
                generation: Some(pin()),
                generation_selector: None,
                top_k: 50,
            },
            semantic_query_text: "scope".to_string(),
            generation: Some(pin()),
            generation_selector: None,
            top_k: 2,
        }),
    };
    if !wait_for_non_error(&socket, &request) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("hybrid count-reached query never became ready".into());
    }

    let response = send_query_request(&socket, &request)?;
    let explanation = match response.payload {
        SearchPlaneQueryIpcResponse::Hybrid(hybrid) => {
            if hybrid.results.len() != 2 {
                shutdown.store(true, Ordering::Release);
                drop(join.join());
                return Err(format!(
                    "expected exactly 2 fused results after top_k cap, got {}",
                    hybrid.results.len()
                )
                .into());
            }
            hybrid.explanation
        }
        other => {
            shutdown.store(true, Ordering::Release);
            drop(join.join());
            return Err(format!("expected Hybrid, got {other:?}").into());
        }
    };
    if explanation.early_stop_reason != Some(EarlyStopReason::CountReached) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!(
            "expected count_reached early_stop_reason, got {:?}",
            explanation.early_stop_reason
        )
        .into());
    }

    stop_runtime(shutdown, join)
}

