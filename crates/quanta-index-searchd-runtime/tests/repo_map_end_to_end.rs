//! End-to-end integration: preload repo-map owner state in searchd runtime and
//! query it through the UDS search-plane IPC.

#![forbid(unsafe_code)]
#![expect(
    clippy::disallowed_methods,
    reason = "test polling paths still use explicit Result fallback checks"
)]
#![expect(
    clippy::indexing_slicing,
    reason = "repo-map assertions intentionally inspect the single top-ranked entry directly"
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

use quanta_index_contract::{
    ManifestGeneration, RepoId, RepoMapActivateGenerationRequest, RepoMapChunkExactness,
    RepoMapChunkRecordDto, RepoMapDocType, RepoMapEdgeKind, RepoMapExactnessSummary,
    RepoMapFileIndexRecord, RepoMapFocusSubjectDto, RepoMapGraphCoverageClass, RepoMapGraphEdgeDto,
    RepoMapItemIndexAvailability, RepoMapQueryRequest, RepoMapRedactionState, RepoMapSourceBundle,
    RepoMapSymbolRecordDto, RevisionId, SearchPlaneControlIpcRequest,
    SearchPlaneControlIpcRequestEnvelope, SearchPlaneControlIpcResponse,
    SearchPlaneControlIpcResponseEnvelope, SearchPlaneIngestIpcRequest,
    SearchPlaneIngestIpcRequestEnvelope, SearchPlaneIngestIpcResponse,
    SearchPlaneIngestIpcResponseEnvelope, SearchPlaneQueryIpcRequest,
    SearchPlaneQueryIpcRequestEnvelope, SearchPlaneQueryIpcResponse,
    SearchPlaneQueryIpcResponseEnvelope,
};
use quanta_index_ipc::send_request;
use quanta_index_searchd::app::SearchdConfig;
use quanta_index_searchd::app::searchd::drive;
use quanta_index_searchd_runtime::build_runtime;

type TestResult = Result<(), Box<dyn Error>>;
static NEXT_SOCKET_ID: AtomicU64 = AtomicU64::new(0);
const READINESS_TIMEOUT: Duration = Duration::from_secs(5);

fn repo() -> RepoId {
    RepoId::new("repo-repomap-e2e")
}

fn revision() -> RevisionId {
    RevisionId::new("rev-repomap-e2e")
}

fn generation() -> ManifestGeneration {
    ManifestGeneration::new(11)
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

fn send_control_request(
    socket: &Path,
    request: &SearchPlaneControlIpcRequestEnvelope,
) -> Result<SearchPlaneControlIpcResponseEnvelope, quanta_index_ipc::IpcError> {
    send_request(socket, request)
}

fn send_ingest_request(
    socket: &Path,
    request: &SearchPlaneIngestIpcRequestEnvelope,
) -> Result<SearchPlaneIngestIpcResponseEnvelope, quanta_index_ipc::IpcError> {
    send_request(socket, request)
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

fn repo_map_bundle() -> RepoMapSourceBundle {
    RepoMapSourceBundle {
        repo_id: repo(),
        revision_id: revision(),
        manifest_generation: generation(),
        snapshot_id: "repomap-snapshot-11".to_string(),
        projection_version: 1,
        authority_digest: "d".repeat(64),
        item_index_availability: RepoMapItemIndexAvailability::Available,
        graph_coverage_class: RepoMapGraphCoverageClass::Complete,
        exactness_summary: RepoMapExactnessSummary::Exact,
        redaction_state: RepoMapRedactionState::Unredacted,
        file_indices: vec![
            RepoMapFileIndexRecord {
                file_identity: "src/lib.rs".to_string(),
                file_path: "src/lib.rs".to_string(),
                file_kind: "library".to_string(),
                line_count: 110,
                symbol_records: vec![RepoMapSymbolRecordDto {
                    subject_identity: "src/lib.rs::Alpha".to_string(),
                    subject_doc_type: RepoMapDocType::Symbol,
                    subject_kind: "struct".to_string(),
                    symbol_name: "Alpha".to_string(),
                    owner_path: "src/lib.rs".to_string(),
                }],
            },
            RepoMapFileIndexRecord {
                file_identity: "src/service/mod.rs".to_string(),
                file_path: "src/service/mod.rs".to_string(),
                file_kind: "service_module".to_string(),
                line_count: 170,
                symbol_records: vec![RepoMapSymbolRecordDto {
                    subject_identity: "src/service/mod.rs::Beta".to_string(),
                    subject_doc_type: RepoMapDocType::Symbol,
                    subject_kind: "service".to_string(),
                    symbol_name: "Beta".to_string(),
                    owner_path: "src/service/mod.rs".to_string(),
                }],
            },
            RepoMapFileIndexRecord {
                file_identity: "tests/repo_map.rs".to_string(),
                file_path: "tests/repo_map.rs".to_string(),
                file_kind: "test".to_string(),
                line_count: 70,
                symbol_records: Vec::new(),
            },
        ],
        call_edges: vec![
            RepoMapGraphEdgeDto {
                from_identity: "src/service/mod.rs::Beta".to_string(),
                to_identity: "src/lib.rs::Alpha".to_string(),
                edge_kind: RepoMapEdgeKind::Call,
            },
            RepoMapGraphEdgeDto {
                from_identity: "src/service/mod.rs::Beta".to_string(),
                to_identity: "tests/repo_map.rs".to_string(),
                edge_kind: RepoMapEdgeKind::Call,
            },
        ],
        import_edges: vec![RepoMapGraphEdgeDto {
            from_identity: "src/service/mod.rs".to_string(),
            to_identity: "src/lib.rs".to_string(),
            edge_kind: RepoMapEdgeKind::Import,
        }],
        chunk_records: vec![
            RepoMapChunkRecordDto {
                subject_identity: "src/lib.rs::Alpha".to_string(),
                owner_path: "src/lib.rs".to_string(),
                token_count: 64,
                preview_text: "Alpha library owner index".to_string(),
                exactness: RepoMapChunkExactness::Exact,
            },
            RepoMapChunkRecordDto {
                subject_identity: "src/service/mod.rs::Beta".to_string(),
                owner_path: "src/service/mod.rs".to_string(),
                token_count: 96,
                preview_text: "Beta service owner query entrypoint".to_string(),
                exactness: RepoMapChunkExactness::Exact,
            },
            RepoMapChunkRecordDto {
                subject_identity: "tests/repo_map.rs".to_string(),
                owner_path: "tests/repo_map.rs".to_string(),
                token_count: 40,
                preview_text: "repo map integration test".to_string(),
                exactness: RepoMapChunkExactness::Approximate,
            },
        ],
    }
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
                subject_identity: "src/service/mod.rs::Beta".to_string(),
                subject_doc_type: RepoMapDocType::Symbol,
            }],
        }),
    }
}

// QI-INT-01: RepoMap bundle ingest now flows over the ingest IPC, not the
// control IPC. Old envelope shape preserved as a helper for the ingest test.
fn repo_map_ingest_envelope() -> SearchPlaneIngestIpcRequestEnvelope {
    SearchPlaneIngestIpcRequestEnvelope {
        request_id: 75,
        payload: SearchPlaneIngestIpcRequest::PublishRepoMapBundle(repo_map_bundle()),
    }
}

fn repo_map_activate_request() -> SearchPlaneControlIpcRequestEnvelope {
    SearchPlaneControlIpcRequestEnvelope {
        request_id: 76,
        payload: SearchPlaneControlIpcRequest::RepoMapActivate(RepoMapActivateGenerationRequest {
            repo_id: repo(),
            revision_id: revision(),
            manifest_generation: generation(),
            manifest_digest: "manifest-digest-11".to_string(),
        }),
    }
}

#[test]
fn repo_map_query_roundtrip_through_searchd_socket() -> TestResult {
    let dir = tempfile::tempdir()?;
    let config = build_config(dir.path());
    let runtime = build_runtime(config)?;
    let query_socket = runtime.query_server.socket_path().to_path_buf();
    let control_socket = runtime.control_server.socket_path().to_path_buf();
    let ingest_socket = runtime.ingest_server.socket_path().to_path_buf();
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_for_drive = Arc::clone(&shutdown);
    let join = thread::Builder::new()
        .name("searchd-repomap-test-driver".into())
        .spawn(move || drive(runtime, shutdown_for_drive))?;

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
    let ingest = send_ingest_request(&ingest_socket, &repo_map_ingest_envelope())?;
    if !matches!(
        ingest.payload,
        SearchPlaneIngestIpcResponse::RepoMapReceipt(_)
    ) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("repo-map ingest did not ack".into());
    }
    let activate = send_control_request(&control_socket, &repo_map_activate_request())?;
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

    let response = send_query_request(&query_socket, &repo_map_request())?;
    let repo_map = match response.payload {
        SearchPlaneQueryIpcResponse::RepoMapQuery(repo_map) => repo_map,
        other => {
            shutdown.store(true, Ordering::Release);
            drop(join.join());
            return Err(format!("expected RepoMapQuery response, got {other:?}").into());
        }
    };
    assert_eq!(repo_map.repo_id, repo());
    assert_eq!(repo_map.revision_id, revision());
    assert_eq!(repo_map.manifest_generation, generation());
    assert_eq!(repo_map.snapshot_meta.snapshot_id, "repomap-snapshot-11");
    assert_eq!(
        repo_map.entries[0].subject_identity,
        "src/service/mod.rs::Beta"
    );
    assert_eq!(repo_map.entries[0].owner_path, "src/service/mod.rs");
    assert_eq!(repo_map.entries[0].subject_doc_type, RepoMapDocType::Symbol);
    assert_eq!(
        repo_map.entries[0].projection_evidence_kind,
        "AuthorityBundle"
    );
    assert_eq!(
        repo_map
            .entries
            .iter()
            .filter(|entry| entry.included)
            .count(),
        1
    );
    assert!(
        repo_map
            .degraded_reason_codes
            .iter()
            .any(|code| code == "token_budget_floor_applied")
    );

    shutdown.store(true, Ordering::Release);
    match join.join() {
        Ok(Ok(())) => Ok(()),
        Ok(Err(err)) => Err(err.into()),
        Err(panic) => Err(format!("driver panic: {panic:?}").into()),
    }
}

#[test]
fn repo_map_query_survives_runtime_restart_from_persisted_state() -> TestResult {
    let dir = tempfile::tempdir()?;
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
        .spawn(move || drive(runtime, shutdown_for_drive))?;

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
    let ingest = send_ingest_request(&ingest_socket, &repo_map_ingest_envelope())?;
    if !matches!(
        ingest.payload,
        SearchPlaneIngestIpcResponse::RepoMapReceipt(_)
    ) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("repo-map ingest did not ack".into());
    }
    let activate = send_control_request(&control_socket, &repo_map_activate_request())?;
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
        .spawn(move || drive(runtime, shutdown_for_drive))?;

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

    let response = send_query_request(&query_socket, &repo_map_request())?;
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
    assert_eq!(repo_map.repo_id, repo());
    assert_eq!(repo_map.revision_id, revision());
    assert_eq!(repo_map.manifest_generation, generation());
    assert_eq!(repo_map.snapshot_meta.snapshot_id, "repomap-snapshot-11");
    assert_eq!(
        repo_map.entries[0].subject_identity,
        "src/service/mod.rs::Beta"
    );

    shutdown.store(true, Ordering::Release);
    match join.join() {
        Ok(Ok(())) => Ok(()),
        Ok(Err(err)) => Err(err.into()),
        Err(panic) => Err(format!("driver panic: {panic:?}").into()),
    }
}

#[test]
fn repo_map_query_without_materialized_snapshot_fails_closed() -> TestResult {
    let dir = tempfile::tempdir()?;
    let config = build_config(dir.path());
    let runtime = build_runtime(config)?;
    let query_socket = runtime.query_server.socket_path().to_path_buf();
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_for_drive = Arc::clone(&shutdown);
    let join = thread::Builder::new()
        .name("searchd-repomap-missing-test-driver".into())
        .spawn(move || drive(runtime, shutdown_for_drive))?;

    if !wait_until(Duration::from_secs(2), || query_socket.exists()) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("socket never appeared".into());
    }

    let response = send_query_request(&query_socket, &repo_map_request())?;
    let err = match response.payload {
        SearchPlaneQueryIpcResponse::Error(err) => err,
        other => {
            shutdown.store(true, Ordering::Release);
            drop(join.join());
            return Err(format!("expected error response, got {other:?}").into());
        }
    };
    assert_eq!(err.code, "NOT_FOUND");
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
    let dir = tempfile::tempdir()?;
    let config = build_config(dir.path());
    let runtime = build_runtime(config)?;
    let query_socket = runtime.query_server.socket_path().to_path_buf();
    let control_socket = runtime.control_server.socket_path().to_path_buf();
    let ingest_socket = runtime.ingest_server.socket_path().to_path_buf();
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_for_drive = Arc::clone(&shutdown);
    let join = thread::Builder::new()
        .name("searchd-repomap-cross-socket-driver".into())
        .spawn(move || drive(runtime, shutdown_for_drive))?;

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

    match send_ingest_request(&query_socket, &repo_map_ingest_envelope()) {
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
