//! End-to-end integration: preload repo-map owner state in searchd runtime and
//! query it through the UDS search-plane IPC.

#![forbid(unsafe_code)]
#![expect(
    clippy::expect_used,
    reason = "integration-test helpers outside `#[test]` fns assert fixture setup with `expect`; the workspace already permits this inside test fns and a helper that cannot set up its fixture has no caller to propagate to"
)]
#![expect(
    clippy::disallowed_methods,
    reason = "test polling paths still use explicit Result fallback checks"
)]
#![expect(
    clippy::panic_in_result_fn,
    reason = "integration tests use Result-returning setup with assertion-style validation"
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

use quanta_index_contract::lex::{LanguageCode, SymbolKindCode};
use quanta_index_contract::{
    FileId, ManifestGeneration, RepoId, RepoMapActivateGenerationRequest, RepoMapChunkExactness,
    RepoMapContainsEdge, RepoMapDocType, RepoMapExactnessSummary, RepoMapFocusSubjectDto,
    RepoMapGraphCoverage, RepoMapGraphCoverageClass, RepoMapItemIndexAvailability, RepoMapNode,
    RepoMapNodeRef, RepoMapOwnsChunkEdge, RepoMapQueryRequest, RepoMapRedactionState,
    RepoMapSourceBundle, RepoRelativePath, RevisionId, SearchPlaneControlIpcRequest,
    SearchPlaneControlIpcRequestEnvelope, SearchPlaneControlIpcResponse,
    SearchPlaneControlIpcResponseEnvelope, SearchPlaneIngestIpcRequest,
    SearchPlaneIngestIpcRequestEnvelope, SearchPlaneIngestIpcResponse,
    SearchPlaneIngestIpcResponseEnvelope, SearchPlaneQueryIpcRequest,
    SearchPlaneQueryIpcRequestEnvelope, SearchPlaneQueryIpcResponse,
    SearchPlaneQueryIpcResponseEnvelope, SymbolId,
};
use quanta_index_ipc::send_request;
use quanta_index_searchd::app::SearchdConfig;
use quanta_index_searchd::app::searchd::drive;
use quanta_index_searchd_runtime::build_runtime;

type TestResult = Result<(), Box<dyn Error>>;
static NEXT_SOCKET_ID: AtomicU64 = AtomicU64::new(0);
const READINESS_TIMEOUT: Duration = Duration::from_secs(5);

fn repo() -> RepoId {
    RepoId::new("repo-repomap-e2e").expect("static fixture ID satisfies canonical policy")
}

fn revision() -> RevisionId {
    RevisionId::new("rev-repomap-e2e").expect("static fixture ID satisfies canonical policy")
}

fn generation() -> ManifestGeneration {
    ManifestGeneration::new(11)
}

fn rust_language() -> Result<LanguageCode, Box<dyn Error>> {
    LanguageCode::new("rust").map_err(|err| -> Box<dyn Error> {
        format!("invalid hard-coded test language code: {err}").into()
    })
}

fn symbol_kind(name: &str) -> Result<SymbolKindCode, Box<dyn Error>> {
    SymbolKindCode::new(name)
        .map_err(|err| format!("invalid hard-coded test symbol kind `{name}`: {err}").into())
}

fn unique_socket_paths() -> (std::path::PathBuf, std::path::PathBuf) {
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let sequence = NEXT_SOCKET_ID.fetch_add(1, Ordering::Relaxed);
    let query =
        std::env::temp_dir().join(format!("qi-repomap-query-{pid}-{nanos}-{sequence}.sock"));
    let control =
        std::env::temp_dir().join(format!("qi-repomap-control-{pid}-{nanos}-{sequence}.sock"));
    (query, control)
}

fn build_config(state_root: &Path) -> SearchdConfig {
    let mut cfg = SearchdConfig::from_state_root(state_root.to_path_buf())
        .try_with_search_corpus_history_retention_limits(
            8,
            16 * 1024 * 1024,
            128,
            256 * 1024 * 1024,
        )
        .expect("valid test retention policy");
    let (query_socket, control_socket) = unique_socket_paths();
    cfg = SearchdConfig::with_socket_overrides(cfg, query_socket, control_socket);
    cfg
}

fn send_query_request(
    socket: &Path,
    request: &SearchPlaneQueryIpcRequestEnvelope,
) -> Result<SearchPlaneQueryIpcResponseEnvelope, quanta_index_ipc::IpcError> {
    send_request(socket, request, quanta_index_ipc::ClientIoPolicy::default())
}

fn send_control_request(
    socket: &Path,
    request: &SearchPlaneControlIpcRequestEnvelope,
) -> Result<SearchPlaneControlIpcResponseEnvelope, quanta_index_ipc::IpcError> {
    send_request(socket, request, quanta_index_ipc::ClientIoPolicy::default())
}

fn send_ingest_request(
    socket: &Path,
    request: &SearchPlaneIngestIpcRequestEnvelope,
) -> Result<SearchPlaneIngestIpcResponseEnvelope, quanta_index_ipc::IpcError> {
    send_request(socket, request, quanta_index_ipc::ClientIoPolicy::default())
}

fn check_connection_fatal(
    err: quanta_index_ipc::IpcError,
) -> Result<(), Box<dyn std::error::Error>> {
    match err {
        quanta_index_ipc::IpcError::Truncated | quanta_index_ipc::IpcError::Io(_) => Ok(()),
        other => Err(format!("expected connection-fatal wrong-socket error, got {other:?}").into()),
    }
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

fn repo_map_bundle() -> Result<RepoMapSourceBundle, Box<dyn Error>> {
    Ok(RepoMapSourceBundle::new(
        repo(),
        revision(),
        generation(),
        "1".repeat(64),
        "repomap-snapshot-11",
        1,
        "d".repeat(64),
        RepoMapGraphCoverage {
            item_index_availability: RepoMapItemIndexAvailability::Available,
            graph_coverage_class: RepoMapGraphCoverageClass::Complete,
        },
        RepoMapExactnessSummary::Exact,
        RepoMapRedactionState::Unredacted,
    )
    .with_node(RepoMapNode::File(quanta_index_contract::RepoMapFileNode {
        file_id: FileId::new("file://src/lib.rs"),
        repo_relative_path: RepoRelativePath::new("src/lib.rs"),
        line_count: 110,
    }))
    .with_node(RepoMapNode::File(quanta_index_contract::RepoMapFileNode {
        file_id: FileId::new("file://src/service/mod.rs"),
        repo_relative_path: RepoRelativePath::new("src/service/mod.rs"),
        line_count: 170,
    }))
    .with_node(RepoMapNode::File(quanta_index_contract::RepoMapFileNode {
        file_id: FileId::new("file://tests/repo_map.rs"),
        repo_relative_path: RepoRelativePath::new("tests/repo_map.rs"),
        line_count: 70,
    }))
    .with_node(RepoMapNode::Symbol(
        quanta_index_contract::RepoMapSymbolNode {
            symbol_id: SymbolId::new("symbol://alpha"),
            owner_path: RepoRelativePath::new("src/lib.rs"),
            local_name: "Alpha".to_string(),
            qualified_name: "src::lib::Alpha".to_string(),
            symbol_kind: symbol_kind("struct")?,
        },
    ))
    .with_node(RepoMapNode::Symbol(
        quanta_index_contract::RepoMapSymbolNode {
            symbol_id: SymbolId::new("symbol://beta"),
            owner_path: RepoRelativePath::new("src/service/mod.rs"),
            local_name: "Beta".to_string(),
            qualified_name: "src::service::Beta".to_string(),
            symbol_kind: symbol_kind("struct")?,
        },
    ))
    .with_node(RepoMapNode::Symbol(
        quanta_index_contract::RepoMapSymbolNode {
            symbol_id: SymbolId::new("symbol://gamma"),
            owner_path: RepoRelativePath::new("tests/repo_map.rs"),
            local_name: "Gamma".to_string(),
            qualified_name: "tests::repo_map::Gamma".to_string(),
            symbol_kind: symbol_kind("struct")?,
        },
    ))
    .with_node(RepoMapNode::Chunk(
        quanta_index_contract::RepoMapChunkNode {
            chunk_id: quanta_index_contract::ChunkId::new("chunk://alpha"),
            owner_path: RepoRelativePath::new("src/lib.rs"),
            language: rust_language()?,
            start_byte: 0,
            end_byte: 128,
            start_line: 1,
            end_line: 12,
            token_count: 64,
            preview_text: "Alpha library owner index".to_string(),
            exactness: RepoMapChunkExactness::Exact,
        },
    ))
    .with_node(RepoMapNode::Chunk(
        quanta_index_contract::RepoMapChunkNode {
            chunk_id: quanta_index_contract::ChunkId::new("chunk://beta"),
            owner_path: RepoRelativePath::new("src/service/mod.rs"),
            language: rust_language()?,
            start_byte: 129,
            end_byte: 256,
            start_line: 13,
            end_line: 28,
            token_count: 96,
            preview_text: "Beta service owner query entrypoint".to_string(),
            exactness: RepoMapChunkExactness::Exact,
        },
    ))
    .with_node(RepoMapNode::Chunk(
        quanta_index_contract::RepoMapChunkNode {
            chunk_id: quanta_index_contract::ChunkId::new("chunk://repomap-test"),
            owner_path: RepoRelativePath::new("tests/repo_map.rs"),
            language: rust_language()?,
            start_byte: 257,
            end_byte: 320,
            start_line: 29,
            end_line: 35,
            token_count: 40,
            preview_text: "repo map integration test".to_string(),
            exactness: RepoMapChunkExactness::Approximate,
        },
    ))
    .with_edge(quanta_index_contract::RepoMapEdge::Contains(
        RepoMapContainsEdge {
            container: RepoMapNodeRef::File(FileId::new("file://src/lib.rs")),
            contained: RepoMapNodeRef::Symbol(SymbolId::new("symbol://alpha")),
        },
    ))
    .with_edge(quanta_index_contract::RepoMapEdge::Contains(
        RepoMapContainsEdge {
            container: RepoMapNodeRef::File(FileId::new("file://src/service/mod.rs")),
            contained: RepoMapNodeRef::Symbol(SymbolId::new("symbol://beta")),
        },
    ))
    .with_edge(quanta_index_contract::RepoMapEdge::Call(
        quanta_index_contract::RepoMapCallEdge {
            caller: RepoMapNodeRef::Symbol(SymbolId::new("symbol://beta")),
            callee: RepoMapNodeRef::Symbol(SymbolId::new("symbol://alpha")),
        },
    ))
    .with_edge(quanta_index_contract::RepoMapEdge::Call(
        quanta_index_contract::RepoMapCallEdge {
            caller: RepoMapNodeRef::Symbol(SymbolId::new("symbol://beta")),
            callee: RepoMapNodeRef::Symbol(SymbolId::new("symbol://gamma")),
        },
    ))
    .with_edge(quanta_index_contract::RepoMapEdge::Import(
        quanta_index_contract::RepoMapImportEdge {
            importer: RepoMapNodeRef::File(FileId::new("file://src/service/mod.rs")),
            imported: RepoMapNodeRef::File(FileId::new("file://src/lib.rs")),
        },
    ))
    .with_edge(quanta_index_contract::RepoMapEdge::OwnsChunk(
        RepoMapOwnsChunkEdge {
            owner: RepoMapNodeRef::Symbol(SymbolId::new("symbol://alpha")),
            chunk: RepoMapNodeRef::Chunk(quanta_index_contract::ChunkId::new("chunk://alpha")),
        },
    ))
    .with_edge(quanta_index_contract::RepoMapEdge::OwnsChunk(
        RepoMapOwnsChunkEdge {
            owner: RepoMapNodeRef::Symbol(SymbolId::new("symbol://beta")),
            chunk: RepoMapNodeRef::Chunk(quanta_index_contract::ChunkId::new("chunk://beta")),
        },
    ))
    .with_edge(quanta_index_contract::RepoMapEdge::OwnsChunk(
        RepoMapOwnsChunkEdge {
            owner: RepoMapNodeRef::File(FileId::new("file://tests/repo_map.rs")),
            chunk: RepoMapNodeRef::Chunk(quanta_index_contract::ChunkId::new(
                "chunk://repomap-test",
            )),
        },
    )))
}

fn repo_map_request() -> SearchPlaneQueryIpcRequestEnvelope {
    SearchPlaneQueryIpcRequestEnvelope {
        request_id: 77,
        payload: SearchPlaneQueryIpcRequest::RepoMapQuery(RepoMapQueryRequest {
            repo_id: repo(),
            revision_id: revision(),
            manifest_generation: generation(),
            query_text: "service owner".to_string(),
            top_k: 1,
            token_budget: 90,
            focus_subjects: vec![RepoMapFocusSubjectDto {
                subject_identity: "symbol://beta".to_string(),
                subject_doc_type: RepoMapDocType::Symbol,
            }],
        }),
    }
}

// QI-INT-01: RepoMap bundle ingest now flows over the ingest IPC, not the
// control IPC. Old envelope shape preserved as a helper for the ingest test.
fn repo_map_ingest_envelope() -> Result<SearchPlaneIngestIpcRequestEnvelope, Box<dyn Error>> {
    Ok(SearchPlaneIngestIpcRequestEnvelope {
        request_id: 75,
        payload: SearchPlaneIngestIpcRequest::PublishRepoMapBundle(repo_map_bundle()?),
    })
}

fn repo_map_activate_request() -> SearchPlaneControlIpcRequestEnvelope {
    SearchPlaneControlIpcRequestEnvelope {
        request_id: 76,
        payload: SearchPlaneControlIpcRequest::RepoMapActivate(RepoMapActivateGenerationRequest {
            repo_id: repo(),
            revision_id: revision(),
            manifest_generation: generation(),
            manifest_digest: "1".repeat(64),
        }),
    }
}

fn assert_repo_map_transport_surface(
    repo_map: &quanta_index_contract::RepoMapQueryResponse,
) -> TestResult {
    if repo_map.repo_id != repo()
        || repo_map.revision_id != revision()
        || repo_map.manifest_generation != generation()
        || repo_map.snapshot_meta.snapshot_id != "repomap-snapshot-11"
        || repo_map.entries.is_empty()
    {
        return Err(format!("unexpected repo-map transport response: {repo_map:?}").into());
    }
    Ok(())
}

#[test]
fn repo_map_query_roundtrip_through_searchd_socket() -> TestResult {
    let dir = quanta_index_searchd_harness::private_tempdir()?;
    let config = build_config(dir.path());
    let runtime = build_runtime(config)?;
    let query_socket = runtime.query_server.socket_path().to_path_buf();
    let control_socket = runtime.control_server.socket_path().to_path_buf();
    let ingest_socket = runtime.ingest_server.socket_path().to_path_buf();
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_for_drive = Arc::clone(&shutdown);
    let join = thread::Builder::new()
        .name("searchd-repomap-test-driver".into())
        .spawn(move || drive(runtime, &shutdown_for_drive))?;

    if !wait_until(Duration::from_secs(2), || query_socket.exists()) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("socket never appeared".into());
    }
    if !wait_until(Duration::from_secs(2), || control_socket.exists()) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("control socket never appeared".into());
    }
    if !wait_until(Duration::from_secs(2), || ingest_socket.exists()) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("ingest socket never appeared".into());
    }
    // QI-INT-01: RepoMap bundle ingest now goes via the ingest socket.
    let ingest = send_ingest_request(&ingest_socket, &repo_map_ingest_envelope()?)
        .map_err(|err| format!("repo-map ingest request failed: {err}"))?;
    if !matches!(
        ingest.payload,
        SearchPlaneIngestIpcResponse::RepoMapReceipt(_)
    ) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("repo-map ingest did not ack".into());
    }
    let activate = send_control_request(&control_socket, &repo_map_activate_request())
        .map_err(|err| format!("repo-map activate request failed: {err}"))?;
    if !matches!(
        activate.payload,
        SearchPlaneControlIpcResponse::RepoMapMutationAck(_)
    ) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("repo-map activate did not ack".into());
    }
    if !wait_until(READINESS_TIMEOUT, || {
        send_query_request(&query_socket, &repo_map_request())
            .map(|response| {
                matches!(
                    response.payload,
                    SearchPlaneQueryIpcResponse::RepoMapQuery(_)
                )
            })
            .unwrap_or(false)
    }) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("repo-map query path never became ready".into());
    }

    let response = send_query_request(&query_socket, &repo_map_request())
        .map_err(|err| format!("repo-map query request failed: {err}"))?;
    let repo_map = match response.payload {
        SearchPlaneQueryIpcResponse::RepoMapQuery(repo_map) => repo_map,
        other => {
            shutdown.store(true, Ordering::Release);
            drop(join.join());
            return Err(format!("expected RepoMapQuery response, got {other:?}").into());
        }
    };
    assert_repo_map_transport_surface(&repo_map)?;

    shutdown.store(true, Ordering::Release);
    match join.join() {
        Ok(Ok(())) => Ok(()),
        Ok(Err(err)) => Err(err.into()),
        Err(panic) => Err(format!("driver panic: {panic:?}").into()),
    }
}

#[test]
fn repo_map_query_survives_runtime_restart_from_persisted_state() -> TestResult {
    let dir = quanta_index_searchd_harness::private_tempdir()?;
    let state_root = dir.path();

    let config = build_config(state_root);
    let runtime = build_runtime(config)?;
    let query_socket = runtime.query_server.socket_path().to_path_buf();
    let control_socket = runtime.control_server.socket_path().to_path_buf();
    let ingest_socket = runtime.ingest_server.socket_path().to_path_buf();
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_for_drive = Arc::clone(&shutdown);
    let join = thread::Builder::new()
        .name("searchd-repomap-persist-seed-driver".into())
        .spawn(move || drive(runtime, &shutdown_for_drive))?;

    if !wait_until(Duration::from_secs(2), || query_socket.exists()) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("socket never appeared".into());
    }
    if !wait_until(Duration::from_secs(2), || control_socket.exists()) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("control socket never appeared".into());
    }
    if !wait_until(Duration::from_secs(2), || ingest_socket.exists()) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("ingest socket never appeared".into());
    }
    let ingest = send_ingest_request(&ingest_socket, &repo_map_ingest_envelope()?)
        .map_err(|err| format!("repo-map ingest request failed: {err}"))?;
    if !matches!(
        ingest.payload,
        SearchPlaneIngestIpcResponse::RepoMapReceipt(_)
    ) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("repo-map ingest did not ack".into());
    }
    let activate = send_control_request(&control_socket, &repo_map_activate_request())
        .map_err(|err| format!("repo-map activate request failed: {err}"))?;
    if !matches!(
        activate.payload,
        SearchPlaneControlIpcResponse::RepoMapMutationAck(_)
    ) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("repo-map activate did not ack".into());
    }

    shutdown.store(true, Ordering::Release);
    match join.join() {
        Ok(Ok(())) => {}
        Ok(Err(err)) => return Err(err.into()),
        Err(panic) => return Err(format!("driver panic: {panic:?}").into()),
    }

    let config = build_config(state_root);
    let runtime = build_runtime(config)?;
    let query_socket = runtime.query_server.socket_path().to_path_buf();
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_for_drive = Arc::clone(&shutdown);
    let join = thread::Builder::new()
        .name("searchd-repomap-persist-restore-driver".into())
        .spawn(move || drive(runtime, &shutdown_for_drive))?;

    if !wait_until(Duration::from_secs(2), || query_socket.exists()) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("socket never appeared after restart".into());
    }
    if !wait_until(READINESS_TIMEOUT, || {
        send_query_request(&query_socket, &repo_map_request())
            .map(|response| {
                matches!(
                    response.payload,
                    SearchPlaneQueryIpcResponse::RepoMapQuery(_)
                )
            })
            .unwrap_or(false)
    }) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("repo-map persisted query path never became ready".into());
    }

    let response = send_query_request(&query_socket, &repo_map_request())
        .map_err(|err| format!("repo-map query request after restart failed: {err}"))?;
    let repo_map = match response.payload {
        SearchPlaneQueryIpcResponse::RepoMapQuery(repo_map) => repo_map,
        other => {
            shutdown.store(true, Ordering::Release);
            drop(join.join());
            return Err(
                format!("expected RepoMapQuery response after restart, got {other:?}").into(),
            );
        }
    };
    assert_repo_map_transport_surface(&repo_map)?;

    shutdown.store(true, Ordering::Release);
    match join.join() {
        Ok(Ok(())) => Ok(()),
        Ok(Err(err)) => Err(err.into()),
        Err(panic) => Err(format!("driver panic: {panic:?}").into()),
    }
}

#[test]
fn repo_map_query_without_materialized_snapshot_fails_closed() -> TestResult {
    let dir = quanta_index_searchd_harness::private_tempdir()?;
    let config = build_config(dir.path());
    let runtime = build_runtime(config)?;
    let query_socket = runtime.query_server.socket_path().to_path_buf();
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_for_drive = Arc::clone(&shutdown);
    let join = thread::Builder::new()
        .name("searchd-repomap-missing-test-driver".into())
        .spawn(move || drive(runtime, &shutdown_for_drive))?;

    if !wait_until(Duration::from_secs(2), || query_socket.exists()) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("socket never appeared".into());
    }

    let response = send_query_request(&query_socket, &repo_map_request())
        .map_err(|err| format!("repo-map missing-snapshot query failed: {err}"))?;
    let err = match response.payload {
        SearchPlaneQueryIpcResponse::Error(err) => err,
        other => {
            shutdown.store(true, Ordering::Release);
            drop(join.join());
            return Err(format!("expected error response, got {other:?}").into());
        }
    };
    assert_eq!(err.code.as_wire_str(), "NOT_FOUND");
    assert!(err.message.contains("no activated generation"));

    shutdown.store(true, Ordering::Release);
    match join.join() {
        Ok(Ok(())) => Ok(()),
        Ok(Err(err)) => Err(err.into()),
        Err(panic) => Err(format!("driver panic: {panic:?}").into()),
    }
}

#[test]
fn cross_socket_requests_fail_closed() -> TestResult {
    let dir = quanta_index_searchd_harness::private_tempdir()?;
    let config = build_config(dir.path());
    let runtime = build_runtime(config)?;
    let query_socket = runtime.query_server.socket_path().to_path_buf();
    let control_socket = runtime.control_server.socket_path().to_path_buf();
    let ingest_socket = runtime.ingest_server.socket_path().to_path_buf();
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_for_drive = Arc::clone(&shutdown);
    let join = thread::Builder::new()
        .name("searchd-repomap-cross-socket-driver".into())
        .spawn(move || drive(runtime, &shutdown_for_drive))?;

    if !wait_until(Duration::from_secs(2), || query_socket.exists()) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("query socket never appeared".into());
    }
    if !wait_until(Duration::from_secs(2), || control_socket.exists()) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("control socket never appeared".into());
    }
    if !wait_until(Duration::from_secs(2), || ingest_socket.exists()) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("ingest socket never appeared".into());
    }

    match send_ingest_request(&query_socket, &repo_map_ingest_envelope()?) {
        Ok(unexpected) => {
            shutdown.store(true, Ordering::Release);
            drop(join.join());
            return Err(format!(
                "ingest envelope on query socket must fail closed, got {unexpected:?}"
            )
            .into());
        }
        Err(err) => check_connection_fatal(err)?,
    }

    match send_query_request(&control_socket, &repo_map_request()) {
        Ok(unexpected) => {
            shutdown.store(true, Ordering::Release);
            drop(join.join());
            return Err(format!(
                "query envelope on control socket must fail closed, got {unexpected:?}"
            )
            .into());
        }
        Err(err) => check_connection_fatal(err)?,
    }

    shutdown.store(true, Ordering::Release);
    match join.join() {
        Ok(Ok(())) => Ok(()),
        Ok(Err(err)) => Err(err.into()),
        Err(panic) => Err(format!("driver panic: {panic:?}").into()),
    }
}
