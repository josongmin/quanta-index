//! SDK-frontdoor end-to-end proof for source-authority packets.

#![forbid(unsafe_code)]
#![expect(
    clippy::disallowed_methods,
    reason = "integration polling uses explicit Result fallback checks"
)]

use std::error::Error;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use quanta_index_contract::lex::LanguageCode;
use quanta_index_contract::{
    ChunkId, ChunkRecord, GenerationPin, ManifestGeneration, RepoId, RevisionId,
    SearchPlaneTrackKind,
};
use quanta_index_sdk::{
    CommitRecord, CommitSha, ConnectOptions, DiffHunkRecord, DirtyBatch, DirtyRecord, LexicalBatch,
    ParseNode, ParseRoleTag, ParseTreeRecord, QuantaIndex, RepoRelativePath, SdkError,
    SearchScopeKey, SearchScopeSurface, StructuralBatch,
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
    CommitSha::from_bytes([
        0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd,
        0xef, 0x01, 0x23, 0x45, 0x67,
    ])
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

fn stop_runtime(shutdown: &Arc<AtomicBool>, join: DriverJoin) -> TestResult {
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
                removed_text: String::new().into_boxed_str(),
                touched_text: "todo!".to_string().into_boxed_str(),
                byte_start: 0,
                byte_end: 5,
            },
        )
}

fn lexical_batch() -> Result<LexicalBatch, Box<dyn Error>> {
    Ok(LexicalBatch::replace_generation(
        repo(),
        revision(),
        generation(),
        "manifest:lexical",
        "batch:lexical",
    )
    .replace_scope(
        SearchScopeKey {
            doc_surface: SearchScopeSurface::File,
            repo_relative_path: RepoRelativePath::new("src/lib.rs"),
        },
        "scope:lexical",
        vec![ChunkRecord {
            chunk_id: ChunkId::new("chunk-dirty"),
            repo_relative_path: RepoRelativePath::new("src/lib.rs"),
            language: rust_language()?,
            start_byte: 0,
            end_byte: 12,
            start_line: 1,
            end_line: 1,
            snippet: "todo!()".to_string().into_boxed_str(),
            indexed_text: "todo!()".to_string().into_boxed_str(),
            text_digest: "text:digest".to_string().into_boxed_str(),
            shape_digest: "shape:digest".to_string().into_boxed_str(),
            structural: None,
            parent_chunk_id: None,
        }],
        Vec::new(),
    ))
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

fn structural_batch() -> Result<StructuralBatch, Box<dyn Error>> {
    Ok(StructuralBatch::replace_generation(
        repo(),
        revision(),
        generation(),
        "manifest:structural",
        "batch:structural",
    )
    .replace_tree(
        structural_scope(),
        "scope:structural",
        ChunkId::new("chunk-tree"),
        ParseTreeRecord {
            wire_version: 1,
            lang: rust_language()?,
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
    ))
}

fn rust_language() -> Result<LanguageCode, Box<dyn Error>> {
    LanguageCode::new("rust").map_err(|err| -> Box<dyn Error> {
        format!("invalid hard-coded test language code: {err}").into()
    })
}

fn structural_scope() -> SearchScopeKey {
    SearchScopeKey {
        doc_surface: SearchScopeSurface::Chunk,
        repo_relative_path: RepoRelativePath::new("src/lib.rs"),
    }
}

fn expect_remote_code(err: SdkError, expected: &str) -> TestResult {
    match err {
        SdkError::Remote { code, .. } if code == expected => Ok(()),
        other @ (SdkError::Usage(_)
        | SdkError::Protocol(_)
        | SdkError::Serialization(_)
        | SdkError::Transport(_)
        | SdkError::Remote { .. }) => {
            Err(format!("expected remote code {expected}, got {other:?}").into())
        }
    }
}

fn wait_for_sdk_ready<T, F>(timeout: Duration, mut run: F) -> Result<T, SdkError>
where
    F: FnMut() -> Result<T, SdkError>,
{
    let start = Instant::now();
    loop {
        match run() {
            Ok(value) => return Ok(value),
            Err(SdkError::Remote { code, message })
                if code == "NOT_READY" && start.elapsed() < timeout =>
            {
                drop(message);
                thread::sleep(Duration::from_millis(10));
            }
            Err(err) => return Err(err),
        }
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
        stop_runtime(&shutdown, join)?;
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
    let structural_receipt = client.structural().publish(&structural_batch()?)?;

    if history_receipt.generation != generation()
        || history_receipt.accepted_replace_scopes != 4
        || history_receipt.accepted_tombstone_scopes != 0
    {
        stop_runtime(&shutdown, join)?;
        return Err(format!("unexpected history receipt: {history_receipt:?}").into());
    }
    if dirty_receipt.generation != generation()
        || dirty_receipt.accepted_replace_scopes != 1
        || dirty_receipt.accepted_tombstone_scopes != 1
    {
        stop_runtime(&shutdown, join)?;
        return Err(format!("unexpected dirty receipt: {dirty_receipt:?}").into());
    }
    if structural_receipt.generation != generation()
        || structural_receipt.accepted_replace_scopes != 1
        || structural_receipt.accepted_tombstone_scopes != 0
    {
        stop_runtime(&shutdown, join)?;
        return Err(format!("unexpected structural receipt: {structural_receipt:?}").into());
    }

    stop_runtime(&shutdown, join)
}

#[test]
fn sdk_query_frontdoor_routes_history_runtime_and_structural_truth() -> TestResult {
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
        stop_runtime(&shutdown, join)?;
        return Err("sdk frontdoor sockets never appeared".into());
    }

    let client = QuantaIndex::connect(
        ConnectOptions::from_state_root(dir.path())
            .with_query_socket(query_socket)
            .with_control_socket(control_socket)
            .with_ingest_socket(ingest_socket),
    )?;

    let _lexical_receipt = client.lexical().publish(&lexical_batch()?)?;
    let _history_receipt = client.history().publish(&history_batch())?;
    let _dirty_receipt = client.runtime().publish_dirty(&dirty_batch())?;
    let _structural_receipt = client.structural().publish(&structural_batch()?)?;
    let _activation = wait_for_sdk_ready(SOCKET_TIMEOUT, || {
        client
            .generations()
            .activate()
            .repo(repo())
            .revision(revision())
            .generation(generation())
            .manifest_digest("manifest:lexical")
            .track(SearchPlaneTrackKind::Lexical)
            .commit()
    })?;

    let history_commit = wait_for_sdk_ready(SOCKET_TIMEOUT, || {
        client
            .history()
            .query()
            .sourcegraph("type:commit author:alice fix")
            .active(repo(), revision())
            .top_k(5)
            .execute()
    })?;
    if history_commit.generation != pin()
        || history_commit.commits.len() != 1
        || !history_commit.diffs.is_empty()
    {
        stop_runtime(&shutdown, join)?;
        return Err(format!("unexpected history commit response: {history_commit:?}").into());
    }
    let commit = history_commit
        .commits
        .first()
        .ok_or_else(|| "missing history commit candidate".to_string())?;
    if commit.author != "alice" || commit.message != "fix: sample" {
        stop_runtime(&shutdown, join)?;
        return Err(format!("unexpected history commit candidate: {commit:?}").into());
    }

    let history_diff = wait_for_sdk_ready(SOCKET_TIMEOUT, || {
        client
            .history()
            .query()
            .native("type:diff todo")
            .active(repo(), revision())
            .top_k(5)
            .execute()
    })?;
    if history_diff.generation != pin()
        || !history_diff.commits.is_empty()
        || history_diff.diffs.len() != 1
    {
        stop_runtime(&shutdown, join)?;
        return Err(format!("unexpected history diff response: {history_diff:?}").into());
    }

    let runtime_query = wait_for_sdk_ready(SOCKET_TIMEOUT, || {
        client
            .runtime()
            .query()
            .sourcegraph("dirty:yes todo")
            .active(repo(), revision())
            .top_k(3)
            .execute()
    })?;
    if runtime_query.generation != pin() || runtime_query.results.len() != 1 {
        stop_runtime(&shutdown, join)?;
        return Err(format!("unexpected runtime response: {runtime_query:?}").into());
    }
    let runtime_candidate = runtime_query
        .results
        .first()
        .ok_or_else(|| "missing runtime candidate".to_string())?;
    if runtime_candidate.candidate_id != "chunk-dirty"
        || runtime_candidate.repo_relative_path.as_str() != "src/lib.rs"
    {
        stop_runtime(&shutdown, join)?;
        return Err(format!("unexpected runtime candidate: {runtime_candidate:?}").into());
    }

    let Err(structural_err) = client
        .structural()
        .query()
        .native("match { :[x] }")
        .active(repo(), revision())
        .top_k(2)
        .execute()
    else {
        stop_runtime(&shutdown, join)?;
        return Err("structural query unexpectedly succeeded".into());
    };
    expect_remote_code(structural_err, "STR_PRODUCER_PARSE_TREE_UNAVAILABLE")?;

    stop_runtime(&shutdown, join)
}
