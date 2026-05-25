//! SDK-frontdoor end-to-end proof for source-authority packets.

#![forbid(unsafe_code)]
#![expect(
    clippy::disallowed_methods,
    reason = "integration polling uses explicit Result fallback checks"
)]
#![expect(
    clippy::panic_in_result_fn,
    reason = "integration tests use Result-returning setup with assertion-style validation"
)]

use std::error::Error;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use quanta_index_contract::lex::LangId;
use quanta_index_contract::{ChunkId, GenerationPin, ManifestGeneration, RepoId, RevisionId};
use quanta_index_sdk::{
    CommitRecord, CommitSha, ConnectOptions, DiffHunkRecord, DirtyBatch, DirtyRecord, ParseNode,
    ParseRoleTag, ParseTreeRecord, QuantaIndex, SdkError, StructuralBatch,
};
use quanta_index_searchd::app::SearchdConfig;
use quanta_index_searchd::app::searchd::drive;
use quanta_index_searchd_runtime::build_runtime;

type TestResult = Result<(), Box<dyn Error>>;
type DriverJoin = thread::JoinHandle<anyhow::Result<()>>;

static NEXT_SOCKET_ID: AtomicU64 = AtomicU64::new(0);
const SOCKET_TIMEOUT: Duration = Duration::from_secs(5);

fn repo() -> RepoId {
    RepoId::new("repo-sdk")
}

fn revision() -> RevisionId {
    RevisionId::new("rev-sdk")
}

fn generation() -> ManifestGeneration {
    ManifestGeneration::new(31)
}

fn pin() -> GenerationPin {
    GenerationPin::new(repo(), revision(), generation())
}

fn commit_sha() -> CommitSha {
    CommitSha::from_hex("0123456789abcdef0123456789abcdef01234567")
        .expect("fixture sha must be valid")
}

fn unique_socket_paths() -> (PathBuf, PathBuf, PathBuf) {
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let sequence = NEXT_SOCKET_ID.fetch_add(1, Ordering::Relaxed);
    let query = std::env::temp_dir().join(format!("qi-sdk-query-{pid}-{nanos}-{sequence}.sock"));
    let control =
        std::env::temp_dir().join(format!("qi-sdk-control-{pid}-{nanos}-{sequence}.sock"));
    let ingest = std::env::temp_dir().join(format!("qi-sdk-ingest-{pid}-{nanos}-{sequence}.sock"));
    (query, control, ingest)
}

fn build_config(state_root: &Path) -> SearchdConfig {
    let (query_socket, control_socket, ingest_socket) = unique_socket_paths();
    SearchdConfig::from_state_root(state_root.to_path_buf())
        .with_socket_overrides(query_socket, control_socket)
        .with_ingest_socket_override(ingest_socket)
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

fn stop_runtime(shutdown: Arc<AtomicBool>, join: DriverJoin) -> TestResult {
    shutdown.store(true, Ordering::Release);
    match join.join() {
        Ok(Ok(())) => Ok(()),
        Ok(Err(err)) => Err(err.into()),
        Err(panic) => Err(format!("driver panic: {panic:?}").into()),
    }
}

fn history_batch() -> quanta_index_sdk::HistoryBatch {
    quanta_index_sdk::HistoryBatch::new(repo(), revision(), generation())
        .commit(CommitRecord {
            wire_version: 1,
            sha: commit_sha(),
            parents: Vec::new(),
            author_time_ms: 11,
            committer_time_ms: 12,
            applied_at_ms: 13,
            author: "alice".to_string().into_boxed_str(),
            committer: "alice".to_string().into_boxed_str(),
            message: "fix: sample".to_string().into_boxed_str(),
            is_merge: false,
            tags: vec!["v1.0.0".to_string().into_boxed_str()],
        })
        .ref_upsert("refs/heads/main", commit_sha())
        .tag_upsert("v1.0.0", commit_sha())
        .diff_hunk(
            commit_sha(),
            "src/lib.rs",
            DiffHunkRecord {
                wire_version: 1,
                hunk_header: "@@ -1,1 +1,2 @@".to_string().into_boxed_str(),
                side: quanta_index_contract::DiffHunkSide::After,
                added_text: "todo!".to_string().into_boxed_str(),
                removed_text: "".to_string().into_boxed_str(),
                touched_text: "todo!".to_string().into_boxed_str(),
                byte_start: 0,
                byte_end: 5,
            },
        )
}

fn dirty_batch() -> DirtyBatch {
    DirtyBatch::new(repo(), revision(), generation())
        .upsert(DirtyRecord {
            wire_version: 1,
            doc_id: ChunkId::new("chunk-dirty"),
            applied_at_ms: 55,
            payload_hash: [7; 32],
        })
        .delete(ChunkId::new("chunk-evict"))
}

fn structural_batch() -> StructuralBatch {
    StructuralBatch::new(repo(), revision(), generation())
        .upsert(
            ChunkId::new("chunk-tree"),
            ParseTreeRecord {
                wire_version: 1,
                lang: LangId::Rust,
                root: ParseNode {
                    kind: "function_item".to_string().into_boxed_str(),
                    byte_start: 0,
                    byte_end: 10,
                    children: Vec::new(),
                },
                source_hash: [9; 32],
                role_tag_schema_version: 1,
                role_tags: vec![ParseRoleTag {
                    role: "expr".to_string().into_boxed_str(),
                    byte_start: 0,
                    byte_end: 4,
                }],
            },
        )
        .delete(ChunkId::new("chunk-drop"))
}

fn expect_remote_code(err: SdkError, expected: &str) -> TestResult {
    match err {
        SdkError::Remote { code, .. } if code == expected => Ok(()),
        other => Err(format!("expected remote code {expected}, got {other:?}").into()),
    }
}

#[test]
fn sdk_publish_frontdoor_routes_history_dirty_and_structural_batches() -> TestResult {
    let dir = tempfile::tempdir()?;
    let runtime = build_runtime(build_config(dir.path()))?;
    let query_socket = runtime.query_server.socket_path().to_path_buf();
    let control_socket = runtime.control_server.socket_path().to_path_buf();
    let ingest_socket = runtime.ingest_server.socket_path().to_path_buf();
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_for_drive = Arc::clone(&shutdown);
    let join = thread::Builder::new()
        .name("sdk-frontdoor-publish".into())
        .spawn(move || drive(runtime, shutdown_for_drive))?;

    if !wait_until(SOCKET_TIMEOUT, || {
        query_socket.exists() && control_socket.exists() && ingest_socket.exists()
    }) {
        stop_runtime(shutdown, join)?;
        return Err("sdk frontdoor sockets never appeared".into());
    }

    let client = QuantaIndex::connect(
        ConnectOptions::from_state_root(dir.path())
            .with_query_socket(query_socket)
            .with_control_socket(control_socket)
            .with_ingest_socket(ingest_socket),
    )?;

    let history_receipt = client.history().publish(&history_batch())?;
    let dirty_receipt = client.runtime().publish_dirty(&dirty_batch())?;
    let structural_receipt = client.structural().publish(&structural_batch())?;

    if history_receipt.first_seq.is_none() || history_receipt.last_seq.is_none() {
        stop_runtime(shutdown, join)?;
        return Err("history receipt missing sequence range".into());
    }
    if dirty_receipt.first_seq.is_none() || dirty_receipt.last_seq.is_none() {
        stop_runtime(shutdown, join)?;
        return Err("dirty receipt missing sequence range".into());
    }
    if structural_receipt.first_seq.is_none() || structural_receipt.last_seq.is_none() {
        stop_runtime(shutdown, join)?;
        return Err("structural receipt missing sequence range".into());
    }

    stop_runtime(shutdown, join)
}

#[test]
fn sdk_query_frontdoor_surfaces_current_fail_closed_codes() -> TestResult {
    let dir = tempfile::tempdir()?;
    let runtime = build_runtime(build_config(dir.path()))?;
    let query_socket = runtime.query_server.socket_path().to_path_buf();
    let control_socket = runtime.control_server.socket_path().to_path_buf();
    let ingest_socket = runtime.ingest_server.socket_path().to_path_buf();
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_for_drive = Arc::clone(&shutdown);
    let join = thread::Builder::new()
        .name("sdk-frontdoor-query".into())
        .spawn(move || drive(runtime, shutdown_for_drive))?;

    if !wait_until(SOCKET_TIMEOUT, || {
        query_socket.exists() && control_socket.exists() && ingest_socket.exists()
    }) {
        stop_runtime(shutdown, join)?;
        return Err("sdk frontdoor sockets never appeared".into());
    }

    let client = QuantaIndex::connect(
        ConnectOptions::from_state_root(dir.path())
            .with_query_socket(query_socket)
            .with_control_socket(control_socket)
            .with_ingest_socket(ingest_socket),
    )?;

    let history_err = client
        .history()
        .query()
        .native("type:commit author:alice")
        .pinned(pin())
        .top_k(5)
        .execute()
        .expect_err("history query should fail closed until history executor lands");
    expect_remote_code(history_err, "HISTORY_PRODUCER_UNAVAILABLE")?;

    let runtime_err = client
        .runtime()
        .query()
        .sourcegraph("dirty:yes")
        .pinned(pin())
        .top_k(3)
        .execute()
        .expect_err("runtime metadata query should fail closed until QI-RT-02 lands");
    expect_remote_code(runtime_err, "NOT_IMPLEMENTED")?;

    let structural_err = client
        .structural()
        .query()
        .native("match { :[x] }")
        .pinned(pin())
        .top_k(2)
        .execute()
        .expect_err("structural query should fail closed until parse-tree executor lands");
    expect_remote_code(structural_err, "STR_PRODUCER_PARSE_TREE_UNAVAILABLE")?;

    stop_runtime(shutdown, join)
}
