//! End-to-end integration: preload repo-map owner state in searchd runtime and
//! query it through the UDS search-plane IPC.

#![forbid(unsafe_code)]

use std::error::Error;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use quanta_index_contract::{
    ManifestGeneration, RepoId, RepoMapActivateGenerationRequestV1, RepoMapFocusSubjectDtoV1,
    RepoMapQueryRequestV1, RepoMapSourceBundleV1, RevisionId, SearchPlaneIpcRequest,
    SearchPlaneIpcRequestEnvelope, SearchPlaneIpcResponse,
};
use quanta_index_ipc::send_request;
use quanta_index_searchd::app::SearchdConfig;
use quanta_index_searchd::app::runtime::SearchdRuntime;
use quanta_index_searchd::app::searchd::drive;

type TestResult = Result<(), Box<dyn Error>>;

fn repo() -> RepoId {
    RepoId::new("repo-repomap-e2e")
}

fn revision() -> RevisionId {
    RevisionId::new("rev-repomap-e2e")
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
    std::env::temp_dir().join(format!("qi-repomap-test-{pid}-{nanos}.sock"))
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

fn repo_map_bundle() -> RepoMapSourceBundleV1 {
    RepoMapSourceBundleV1 {
        repo_id: repo(),
        revision_id: revision(),
        manifest_generation: generation(),
        snapshot_id: "repomap-snapshot-11".to_string(),
        projection_version: 1,
        authority_digest: "d".repeat(64),
        item_index_availability: "available".to_string(),
        graph_coverage_class: "complete".to_string(),
        exactness_summary: "bootstrap-owner".to_string(),
        entry_identities: vec![
            "src/lib.rs::Alpha".to_string(),
            "src/service/mod.rs::Beta".to_string(),
            "tests/repo_map.rs::Gamma".to_string(),
        ],
    }
}

fn repo_map_request() -> SearchPlaneIpcRequestEnvelope {
    SearchPlaneIpcRequestEnvelope {
        request_id: 77,
        payload: SearchPlaneIpcRequest::RepoMapQuery(RepoMapQueryRequestV1 {
            repo_id: repo(),
            revision_id: revision(),
            manifest_generation: generation(),
            query_text: "service owner".to_string(),
            top_k: 5,
            token_budget: 256,
            focus_subjects: vec![RepoMapFocusSubjectDtoV1 {
                subject_identity: "src/service/mod.rs::Beta".to_string(),
                subject_doc_type: "Symbol".to_string(),
            }],
        }),
    }
}

fn repo_map_ingest_request() -> SearchPlaneIpcRequestEnvelope {
    SearchPlaneIpcRequestEnvelope {
        request_id: 75,
        payload: SearchPlaneIpcRequest::RepoMapIngest(repo_map_bundle()),
    }
}

fn repo_map_activate_request() -> SearchPlaneIpcRequestEnvelope {
    SearchPlaneIpcRequestEnvelope {
        request_id: 76,
        payload: SearchPlaneIpcRequest::RepoMapActivate(RepoMapActivateGenerationRequestV1 {
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
    let runtime = SearchdRuntime::build(config)?;
    let socket = runtime.query_server.socket_path().to_path_buf();
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_for_drive = Arc::clone(&shutdown);
    let join = thread::Builder::new()
        .name("searchd-repomap-test-driver".into())
        .spawn(move || drive(runtime, shutdown_for_drive))?;

    if !wait_until(Duration::from_secs(2), || socket.exists()) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("socket never appeared".into());
    }
    let ingest = send_request(&socket, &repo_map_ingest_request())?;
    if !matches!(ingest.payload, SearchPlaneIpcResponse::RepoMapMutationAck(_)) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("repo-map ingest did not ack".into());
    }
    let activate = send_request(&socket, &repo_map_activate_request())?;
    if !matches!(activate.payload, SearchPlaneIpcResponse::RepoMapMutationAck(_)) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("repo-map activate did not ack".into());
    }
    if !wait_until(Duration::from_secs(2), || {
        send_request(&socket, &repo_map_request())
            .map(|response| matches!(response.payload, SearchPlaneIpcResponse::RepoMapQuery(_)))
            .unwrap_or(false)
    }) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("repo-map query path never became ready".into());
    }

    let response = send_request(&socket, &repo_map_request())?;
    let repo_map = match response.payload {
        SearchPlaneIpcResponse::RepoMapQuery(repo_map) => repo_map,
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
    assert_eq!(repo_map.entries.len(), 1);
    assert_eq!(
        repo_map.entries[0].subject_identity,
        "src/service/mod.rs::Beta"
    );
    assert_eq!(repo_map.entries[0].owner_path, "src/service/mod.rs");
    assert_eq!(repo_map.entries[0].subject_doc_type, "Symbol");
    assert_eq!(repo_map.entries[0].projection_evidence_kind, "ParserItemIndex");

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
    let runtime = SearchdRuntime::build(config)?;
    let socket = runtime.query_server.socket_path().to_path_buf();
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_for_drive = Arc::clone(&shutdown);
    let join = thread::Builder::new()
        .name("searchd-repomap-persist-seed-driver".into())
        .spawn(move || drive(runtime, shutdown_for_drive))?;

    if !wait_until(Duration::from_secs(2), || socket.exists()) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("socket never appeared".into());
    }
    let ingest = send_request(&socket, &repo_map_ingest_request())?;
    if !matches!(ingest.payload, SearchPlaneIpcResponse::RepoMapMutationAck(_)) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("repo-map ingest did not ack".into());
    }
    let activate = send_request(&socket, &repo_map_activate_request())?;
    if !matches!(activate.payload, SearchPlaneIpcResponse::RepoMapMutationAck(_)) {
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
    let runtime = SearchdRuntime::build(config)?;
    let socket = runtime.query_server.socket_path().to_path_buf();
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_for_drive = Arc::clone(&shutdown);
    let join = thread::Builder::new()
        .name("searchd-repomap-persist-restore-driver".into())
        .spawn(move || drive(runtime, shutdown_for_drive))?;

    if !wait_until(Duration::from_secs(2), || socket.exists()) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("socket never appeared after restart".into());
    }
    if !wait_until(Duration::from_secs(2), || {
        send_request(&socket, &repo_map_request())
            .map(|response| matches!(response.payload, SearchPlaneIpcResponse::RepoMapQuery(_)))
            .unwrap_or(false)
    }) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("repo-map persisted query path never became ready".into());
    }

    let response = send_request(&socket, &repo_map_request())?;
    let repo_map = match response.payload {
        SearchPlaneIpcResponse::RepoMapQuery(repo_map) => repo_map,
        other => {
            shutdown.store(true, Ordering::Release);
            drop(join.join());
            return Err(format!("expected RepoMapQuery response after restart, got {other:?}").into());
        }
    };
    assert_eq!(repo_map.repo_id, repo());
    assert_eq!(repo_map.revision_id, revision());
    assert_eq!(repo_map.manifest_generation, generation());
    assert_eq!(repo_map.snapshot_meta.snapshot_id, "repomap-snapshot-11");
    assert_eq!(repo_map.entries.len(), 1);
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
    let runtime = SearchdRuntime::build(config)?;
    let socket = runtime.query_server.socket_path().to_path_buf();
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_for_drive = Arc::clone(&shutdown);
    let join = thread::Builder::new()
        .name("searchd-repomap-missing-test-driver".into())
        .spawn(move || drive(runtime, shutdown_for_drive))?;

    if !wait_until(Duration::from_secs(2), || socket.exists()) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("socket never appeared".into());
    }

    let response = send_request(&socket, &repo_map_request())?;
    let err = match response.payload {
        SearchPlaneIpcResponse::Error(err) => err,
        other => {
            shutdown.store(true, Ordering::Release);
            drop(join.join());
            return Err(format!("expected Error response, got {other:?}").into());
        }
    };
    assert_eq!(err.code, "NOT_FOUND");
    assert!(err.message.contains("repomap snapshot missing"));

    shutdown.store(true, Ordering::Release);
    match join.join() {
        Ok(Ok(())) => Ok(()),
        Ok(Err(err)) => Err(err.into()),
        Err(panic) => Err(format!("driver panic: {panic:?}").into()),
    }
}
