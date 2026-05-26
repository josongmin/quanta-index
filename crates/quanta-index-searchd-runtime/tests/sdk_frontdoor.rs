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

use quanta_index_contract::lex::{
    LanguageCode, SymbolKindCode, SymbolKindFamily, SymbolRecord, SymbolRelationship, SymbolSpan,
    compute_parse_tree_source_hash,
};
use quanta_index_contract::{
    ChunkId, ChunkRecord, GenerationPin, ManifestGeneration, RepoId, RevisionId,
    SearchPlaneTrackKind, SymbolId,
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
    quanta_index_sdk::HistoryBatch::new(repo(), revision(), generation(), "batch:history-sdk")
        .manifest_digest("manifest:history-sdk")
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
        vec![
            ChunkRecord {
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
            },
            ChunkRecord {
                chunk_id: ChunkId::new("chunk-tree"),
                repo_relative_path: RepoRelativePath::new("src/lib.rs"),
                language: rust_language()?,
                start_byte: 0,
                end_byte: 12,
                start_line: 1,
                end_line: 1,
                snippet: "fn main() {}".to_string().into_boxed_str(),
                indexed_text: "fn main() {}".to_string().into_boxed_str(),
                text_digest: "text:tree".to_string().into_boxed_str(),
                shape_digest: "shape:tree".to_string().into_boxed_str(),
                structural: None,
                parent_chunk_id: None,
            },
        ],
        vec![symbol_record()?],
    ))
}

fn symbol_record() -> Result<SymbolRecord, Box<dyn Error>> {
    Ok(SymbolRecord {
        symbol_id: SymbolId::new("sym-sdk"),
        repo_relative_path: RepoRelativePath::new("src/lib.rs"),
        language: rust_language()?,
        symbol_kind: SymbolKindCode::new("function").map_err(|err| -> Box<dyn Error> {
            format!("invalid symbol kind code: {err}").into()
        })?,
        symbol_kind_family: Some(SymbolKindFamily::Callable),
        local_name: "MySdkSymbol".into(),
        qualified_name: "crate::MySdkSymbol".to_string().into_boxed_str(),
        signature: None,
        visibility: None,
        definition_span: SymbolSpan {
            path: "src/lib.rs".to_string().into_boxed_str(),
            byte_start: 0,
            byte_end: 12,
            line_start: 1,
            line_end: 1,
        },
        container_qualified_name: Some("crate".to_string().into_boxed_str()),
        relationship: SymbolRelationship::Def,
    })
}

fn dirty_batch() -> DirtyBatch {
    DirtyBatch::new(
        repo(),
        revision(),
        generation(),
        1_717_171_717_000,
        "batch:dirty-sdk",
    )
    .upsert(DirtyRecord {
        wire_version: 1,
        doc_id: ChunkId::new("chunk-dirty"),
        applied_at_ms: 55,
        payload_hash: [7; 32],
    })
    .delete(ChunkId::new("chunk-evict"))
}

fn structural_batch_with_chunk(chunk_id: ChunkId) -> Result<StructuralBatch, Box<dyn Error>> {
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
        chunk_id,
        ParseTreeRecord {
            wire_version: 1,
            lang: rust_language()?,
            root: ParseNode {
                kind: "function_item".to_string().into_boxed_str(),
                byte_start: 0,
                byte_end: 10,
                children: vec![
                    ParseNode {
                        kind: "identifier".to_string().into_boxed_str(),
                        byte_start: 3,
                        byte_end: 7,
                        children: Vec::new(),
                    },
                    ParseNode {
                        kind: "block".to_string().into_boxed_str(),
                        byte_start: 8,
                        byte_end: 10,
                        children: Vec::new(),
                    },
                ],
            },
            source_hash: compute_parse_tree_source_hash("fn main() {}"),
            role_tag_schema_version: 1,
            role_tags: vec![ParseRoleTag {
                role: "expr".to_string().into_boxed_str(),
                byte_start: 0,
                byte_end: 4,
            }],
        },
    ))
}

fn structural_batch() -> Result<StructuralBatch, Box<dyn Error>> {
    structural_batch_with_chunk(ChunkId::new("chunk-tree"))
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

fn wait_for_sdk_observation<T, F, P>(
    timeout: Duration,
    mut run: F,
    mut ready: P,
) -> Result<T, SdkError>
where
    F: FnMut() -> Result<T, SdkError>,
    P: FnMut(&T) -> bool,
{
    let start = Instant::now();
    loop {
        match run() {
            Ok(value) if ready(&value) || start.elapsed() >= timeout => return Ok(value),
            Ok(_value) => thread::sleep(Duration::from_millis(10)),
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

fn assert_single_symbol_candidate(
    response: &quanta_index_contract::SymbolQueryResponse,
) -> TestResult {
    if response.generation != pin() || response.results.len() != 1 {
        return Err(format!("unexpected symbol response: {response:?}").into());
    }
    let symbol_candidate = response
        .results
        .first()
        .ok_or_else(|| "missing symbol candidate".to_string())?;
    if symbol_candidate.candidate_id != "sym-sdk"
        || symbol_candidate.repo_relative_path.as_str() != "src/lib.rs"
        || !symbol_candidate.snippet.contains("MySdkSymbol")
        || symbol_candidate.symbol_kind.as_str() != "function"
        || symbol_candidate.symbol_kind_family != Some(SymbolKindFamily::Callable)
    {
        return Err(format!("unexpected symbol candidate: {symbol_candidate:?}").into());
    }
    Ok(())
}

fn wait_for_symbol_query<F>(
    timeout: Duration,
    run: F,
) -> Result<quanta_index_contract::SymbolQueryResponse, SdkError>
where
    F: FnMut() -> Result<quanta_index_contract::SymbolQueryResponse, SdkError>,
{
    wait_for_sdk_observation(timeout, run, |response| {
        response.generation == pin() && response.results.len() == 1
    })
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

    let lexical_receipt = client.lexical().publish(&lexical_batch()?)?;
    let history_receipt = client.history().publish(&history_batch())?;
    let dirty_receipt = client.runtime().publish_dirty(&dirty_batch())?;
    let structural_receipt = client.structural().publish(&structural_batch()?)?;

    if lexical_receipt.generation != generation()
        || lexical_receipt.accepted_replace_scopes != 1
        || lexical_receipt.accepted_tombstone_scopes != 0
    {
        stop_runtime(&shutdown, join)?;
        return Err(format!("unexpected lexical receipt: {lexical_receipt:?}").into());
    }
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
            .manifest_digest("manifest:history-sdk")
            .tracks([
                SearchPlaneTrackKind::Lexical,
                SearchPlaneTrackKind::Structural,
            ])
            .commit()
    })?;

    let history_commit = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .history()
                .query()
                .sourcegraph("type:commit rev:refs/heads/main author:alice fix")
                .active(repo(), revision())
                .top_k(5)
                .execute()
        },
        |response| {
            response.generation == pin() && response.commits.len() == 1 && response.diffs.is_empty()
        },
    )?;
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

    let history_diff = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .history()
                .query()
                .native("type:diff todo")
                .active(repo(), revision())
                .top_k(5)
                .execute()
        },
        |response| {
            response.generation == pin() && response.commits.is_empty() && response.diffs.len() == 1
        },
    )?;
    if history_diff.generation != pin()
        || !history_diff.commits.is_empty()
        || history_diff.diffs.len() != 1
    {
        stop_runtime(&shutdown, join)?;
        return Err(format!("unexpected history diff response: {history_diff:?}").into());
    }

    let symbol_select_native = wait_for_symbol_query(SOCKET_TIMEOUT, || {
        client
            .symbol()
            .query()
            .native("select:symbol MySdkSymbol")
            .active(repo(), revision())
            .top_k(3)
            .execute()
    })?;
    assert_single_symbol_candidate(&symbol_select_native)?;

    let symbol_type_native = wait_for_symbol_query(SOCKET_TIMEOUT, || {
        client
            .symbol()
            .query()
            .native("type:symbol MySdkSymbol")
            .active(repo(), revision())
            .top_k(3)
            .execute()
    })?;
    assert_single_symbol_candidate(&symbol_type_native)?;

    let symbol_select_sourcegraph = wait_for_symbol_query(SOCKET_TIMEOUT, || {
        client
            .symbol()
            .query()
            .sourcegraph("select:symbol MySdkSymbol")
            .active(repo(), revision())
            .top_k(3)
            .execute()
    })?;
    assert_single_symbol_candidate(&symbol_select_sourcegraph)?;

    let symbol_type_sourcegraph = wait_for_symbol_query(SOCKET_TIMEOUT, || {
        client
            .symbol()
            .query()
            .sourcegraph("type:symbol MySdkSymbol")
            .active(repo(), revision())
            .top_k(3)
            .execute()
    })?;
    assert_single_symbol_candidate(&symbol_type_sourcegraph)?;

    let runtime_query = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .runtime()
                .query()
                .sourcegraph("dirty:yes todo")
                .active(repo(), revision())
                .top_k(3)
                .execute()
        },
        |response| response.generation == pin() && response.results.len() == 1,
    )?;
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

    let structural_query = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .structural()
                .query()
                .native("match { :[x] }")
                .active(repo(), revision())
                .top_k(2)
                .execute()
        },
        |response| response.generation == pin() && response.results.len() == 1,
    )?;
    if structural_query.generation != pin() || structural_query.results.len() != 1 {
        stop_runtime(&shutdown, join)?;
        return Err(format!("unexpected structural response: {structural_query:?}").into());
    }
    let structural_candidate = structural_query
        .results
        .first()
        .ok_or_else(|| "missing structural candidate".to_string())?;
    if structural_candidate.candidate_id != "chunk-tree" || structural_candidate.bindings.len() != 1
    {
        stop_runtime(&shutdown, join)?;
        return Err(format!("unexpected structural candidate: {structural_candidate:?}").into());
    }
    let structural_binding = structural_candidate
        .bindings
        .first()
        .ok_or_else(|| "missing structural binding".to_string())?;
    if structural_binding.metavariable != "x"
        || structural_binding.start_byte != 0
        || structural_binding.end_byte != 10
    {
        stop_runtime(&shutdown, join)?;
        return Err(format!("unexpected structural binding: {structural_binding:?}").into());
    }

    let structural_pinned_query = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .structural()
                .query()
                .native("match { :[x] }")
                .pinned(pin())
                .top_k(2)
                .execute()
        },
        |response| response.generation == pin() && response.results.len() == 1,
    )?;
    if structural_pinned_query.generation != pin() || structural_pinned_query.results.len() != 1 {
        stop_runtime(&shutdown, join)?;
        return Err(
            format!("unexpected pinned structural response: {structural_pinned_query:?}").into(),
        );
    }
    let structural_pinned_candidate = structural_pinned_query
        .results
        .first()
        .ok_or_else(|| "missing pinned structural candidate".to_string())?;
    if structural_pinned_candidate.candidate_id != "chunk-tree"
        || structural_pinned_candidate.bindings.len() != 1
    {
        stop_runtime(&shutdown, join)?;
        return Err(format!(
            "unexpected pinned structural candidate: {structural_pinned_candidate:?}"
        )
        .into());
    }

    let structural_root_kind = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .structural()
                .query()
                .native("match { function_item }")
                .active(repo(), revision())
                .top_k(2)
                .execute()
        },
        |response| response.generation == pin() && response.results.len() == 1,
    )?;
    if structural_root_kind.generation != pin() || structural_root_kind.results.len() != 1 {
        stop_runtime(&shutdown, join)?;
        return Err(
            format!("unexpected structural root-kind response: {structural_root_kind:?}").into(),
        );
    }
    let structural_root_kind_candidate = structural_root_kind
        .results
        .first()
        .ok_or_else(|| "missing structural root-kind candidate".to_string())?;
    if structural_root_kind_candidate.candidate_id != "chunk-tree"
        || !structural_root_kind_candidate.bindings.is_empty()
    {
        stop_runtime(&shutdown, join)?;
        return Err(format!(
            "unexpected structural root-kind candidate: {structural_root_kind_candidate:?}"
        )
        .into());
    }

    let structural_root_kind_capture = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .structural()
                .query()
                .native("match { function_item :[x] }")
                .active(repo(), revision())
                .top_k(2)
                .execute()
        },
        |response| response.generation == pin() && response.results.len() == 1,
    )?;
    if structural_root_kind_capture.generation != pin()
        || structural_root_kind_capture.results.len() != 1
    {
        stop_runtime(&shutdown, join)?;
        return Err(format!(
            "unexpected structural root-kind+capture response: {structural_root_kind_capture:?}"
        )
        .into());
    }
    let structural_root_kind_capture_candidate = structural_root_kind_capture
        .results
        .first()
        .ok_or_else(|| "missing structural root-kind+capture candidate".to_string())?;
    if structural_root_kind_capture_candidate.candidate_id != "chunk-tree"
        || structural_root_kind_capture_candidate.bindings.len() != 1
    {
        stop_runtime(&shutdown, join)?;
        return Err(format!(
            "unexpected structural root-kind+capture candidate: \
             {structural_root_kind_capture_candidate:?}"
        )
        .into());
    }
    let structural_root_kind_capture_binding = structural_root_kind_capture_candidate
        .bindings
        .first()
        .ok_or_else(|| "missing structural root-kind+capture binding".to_string())?;
    if structural_root_kind_capture_binding.metavariable != "x"
        || structural_root_kind_capture_binding.start_byte != 0
        || structural_root_kind_capture_binding.end_byte != 10
    {
        stop_runtime(&shutdown, join)?;
        return Err(format!(
            "unexpected structural root-kind+capture binding: \
             {structural_root_kind_capture_binding:?}"
        )
        .into());
    }

    let structural_child_capture = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .structural()
                .query()
                .native("match { function_item { { identifier :[name] } } }")
                .active(repo(), revision())
                .top_k(2)
                .execute()
        },
        |response| response.generation == pin() && response.results.len() == 1,
    )?;
    if structural_child_capture.generation != pin() || structural_child_capture.results.len() != 1 {
        stop_runtime(&shutdown, join)?;
        return Err(format!(
            "unexpected structural child-capture response: {structural_child_capture:?}"
        )
        .into());
    }
    let structural_child_capture_candidate = structural_child_capture
        .results
        .first()
        .ok_or_else(|| "missing structural child-capture candidate".to_string())?;
    let structural_child_capture_binding = structural_child_capture_candidate
        .bindings
        .first()
        .ok_or_else(|| "missing structural child-capture binding".to_string())?;
    if structural_child_capture_candidate.candidate_id != "chunk-tree"
        || structural_child_capture_binding.metavariable != "name"
        || structural_child_capture_binding.start_byte != 3
        || structural_child_capture_binding.end_byte != 7
    {
        stop_runtime(&shutdown, join)?;
        return Err(format!(
            "unexpected structural child-capture candidate/binding: \
             {structural_child_capture_candidate:?}"
        )
        .into());
    }

    let structural_where_inside_outside = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .structural()
                .query()
                .native(
                    "match { identifier :[name] where :[name] == \"main\" inside { function_item } outside { trait_item } }",
                )
                .active(repo(), revision())
                .top_k(2)
                .execute()
        },
        |response| response.generation == pin() && response.results.len() == 1,
    )?;
    let structural_where_inside_outside_candidate = structural_where_inside_outside
        .results
        .first()
        .ok_or_else(|| "missing structural where/inside/outside candidate".to_string())?;
    let structural_where_inside_outside_binding = structural_where_inside_outside_candidate
        .bindings
        .first()
        .ok_or_else(|| "missing structural where/inside/outside binding".to_string())?;
    if structural_where_inside_outside_candidate.candidate_id != "chunk-tree"
        || structural_where_inside_outside_binding.metavariable != "name"
        || structural_where_inside_outside_binding.start_byte != 3
        || structural_where_inside_outside_binding.end_byte != 7
    {
        stop_runtime(&shutdown, join)?;
        return Err(format!(
            "unexpected structural where/inside/outside candidate/binding: \
             {structural_where_inside_outside_candidate:?}"
        )
        .into());
    }

    let structural_variadic = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .structural()
                .query()
                .native("match { function_item { :[...prefix] block } }")
                .active(repo(), revision())
                .top_k(2)
                .execute()
        },
        |response| response.generation == pin() && response.results.len() == 1,
    )?;
    let structural_variadic_candidate = structural_variadic
        .results
        .first()
        .ok_or_else(|| "missing structural variadic candidate".to_string())?;
    let structural_variadic_binding = structural_variadic_candidate
        .bindings
        .first()
        .ok_or_else(|| "missing structural variadic binding".to_string())?;
    if structural_variadic_candidate.candidate_id != "chunk-tree"
        || structural_variadic_binding.metavariable != "prefix"
        || structural_variadic_binding.start_byte != 3
        || structural_variadic_binding.end_byte != 7
    {
        stop_runtime(&shutdown, join)?;
        return Err(format!(
            "unexpected structural variadic candidate/binding: {structural_variadic_candidate:?}"
        )
        .into());
    }

    let structural_filtered_native = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .structural()
                .query()
                .native(
                    "repo:repo-sdk file:src/lib.rs lang:rust match { function_item { { identifier :[name] } } }",
                )
                .active(repo(), revision())
                .top_k(2)
                .execute()
        },
        |response| response.generation == pin() && response.results.len() == 1,
    )?;
    if structural_filtered_native.generation != pin()
        || structural_filtered_native.results.len() != 1
    {
        stop_runtime(&shutdown, join)?;
        return Err(format!(
            "unexpected filtered native structural response: {structural_filtered_native:?}"
        )
        .into());
    }
    let structural_filtered_native_candidate = structural_filtered_native
        .results
        .first()
        .ok_or_else(|| "missing filtered native structural candidate".to_string())?;
    let structural_filtered_native_binding = structural_filtered_native_candidate
        .bindings
        .first()
        .ok_or_else(|| "missing filtered native structural binding".to_string())?;
    if structural_filtered_native_candidate.candidate_id != "chunk-tree"
        || structural_filtered_native_binding.metavariable != "name"
        || structural_filtered_native_binding.start_byte != 3
        || structural_filtered_native_binding.end_byte != 7
    {
        stop_runtime(&shutdown, join)?;
        return Err(format!(
            "unexpected filtered native structural candidate/binding: \
             {structural_filtered_native_candidate:?}"
        )
        .into());
    }

    let Err(structural_err) = client
        .structural()
        .query()
        .native("lang:java match { :[x] }")
        .active(repo(), revision())
        .top_k(2)
        .execute()
    else {
        stop_runtime(&shutdown, join)?;
        return Err("unsupported-lang structural query unexpectedly succeeded".into());
    };
    expect_remote_code(structural_err, "STR_LANG_NOT_SUPPORTED")?;

    let structural_file_query = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .structural()
                .query()
                .native("file:src/lib.rs match { :[x] }")
                .active(repo(), revision())
                .top_k(2)
                .execute()
        },
        |response| response.generation == pin() && response.results.len() == 1,
    )?;
    if structural_file_query.generation != pin() || structural_file_query.results.len() != 1 {
        stop_runtime(&shutdown, join)?;
        return Err(
            format!("unexpected structural file response: {structural_file_query:?}").into(),
        );
    }

    let structural_repo_query = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .structural()
                .query()
                .native("repo:repo-sdk match { :[x] }")
                .active(repo(), revision())
                .top_k(2)
                .execute()
        },
        |response| response.generation == pin() && response.results.len() == 1,
    )?;
    if structural_repo_query.generation != pin() || structural_repo_query.results.len() != 1 {
        stop_runtime(&shutdown, join)?;
        return Err(
            format!("unexpected structural repo response: {structural_repo_query:?}").into(),
        );
    }

    let structural_repo_file_lang_query = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .structural()
                .query()
                .native("repo:repo-sdk file:src/lib.rs lang:rust match { function_item :[x] }")
                .active(repo(), revision())
                .top_k(2)
                .execute()
        },
        |response| response.generation == pin() && response.results.len() == 1,
    )?;
    if structural_repo_file_lang_query.generation != pin()
        || structural_repo_file_lang_query.results.len() != 1
    {
        stop_runtime(&shutdown, join)?;
        return Err(format!(
            "unexpected structural repo+file+lang response: \
                 {structural_repo_file_lang_query:?}"
        )
        .into());
    }
    let structural_repo_file_lang_candidate = structural_repo_file_lang_query
        .results
        .first()
        .ok_or_else(|| "missing structural repo+file+lang candidate".to_string())?;
    if structural_repo_file_lang_candidate.candidate_id != "chunk-tree"
        || structural_repo_file_lang_candidate.bindings.len() != 1
    {
        stop_runtime(&shutdown, join)?;
        return Err(format!(
            "unexpected structural repo+file+lang candidate: \
             {structural_repo_file_lang_candidate:?}"
        )
        .into());
    }

    let structural_repo_miss = client
        .structural()
        .query()
        .native("repo:other-repo match { :[x] }")
        .active(repo(), revision())
        .top_k(2)
        .execute()?;
    if structural_repo_miss.generation != pin() || !structural_repo_miss.results.is_empty() {
        stop_runtime(&shutdown, join)?;
        return Err(
            format!("unexpected structural repo-miss response: {structural_repo_miss:?}").into(),
        );
    }

    let Err(structural_invalid_request_err) = client
        .structural()
        .query()
        .native("select:repo match { :[x] }")
        .active(repo(), revision())
        .top_k(2)
        .execute()
    else {
        stop_runtime(&shutdown, join)?;
        return Err("invalid-filter structural query unexpectedly succeeded".into());
    };
    expect_remote_code(structural_invalid_request_err, "STR_INVALID_REQUEST")?;

    let structural_native_pinned = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .structural()
                .query()
                .native("match { function_item }")
                .pinned(pin())
                .top_k(2)
                .execute()
        },
        |response| response.generation == pin() && response.results.len() == 1,
    )?;
    if structural_native_pinned.generation != pin() || structural_native_pinned.results.len() != 1 {
        stop_runtime(&shutdown, join)?;
        return Err(format!(
            "unexpected pinned structural native response: {structural_native_pinned:?}"
        )
        .into());
    }
    let structural_native_pinned_candidate = structural_native_pinned
        .results
        .first()
        .ok_or_else(|| "missing pinned structural native candidate".to_string())?;
    if structural_native_pinned_candidate.candidate_id != "chunk-tree" {
        stop_runtime(&shutdown, join)?;
        return Err(format!(
            "unexpected pinned structural native candidate: \
             {structural_native_pinned_candidate:?}"
        )
        .into());
    }

    stop_runtime(&shutdown, join)
}
