//! End-to-end integration: publisher writes ops → searchd dispatcher consumes
//! and feeds adapters → searchd UDS query server returns matches.

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
    clippy::wildcard_enum_match_arm,
    reason = "integration response checks intentionally collapse non-target variants"
)]
#![expect(
    dead_code,
    reason = "mixed migration: typed ingest helpers land before all channel setup blocks are cut over"
)]

#[path = "common/frontdoor_scenarios.rs"]
mod frontdoor_scenarios;

use std::collections::BTreeMap;
use std::error::Error;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use quanta_index_contract::lex::{
    CommitRecord, CommitSha, LanguageCode, ParseNode, ParseRoleTag, ParseTreeRecord,
    compute_parse_tree_source_hash,
};
use quanta_index_contract::{
    BatchIngestMode, ChunkId, ChunkRecord, GenerationPin, HistoryIngestBatch, HistoryQueryRequest,
    HybridQueryRequest, LqVisibility, ManifestGeneration, RepoId, RepoRelativePath, RevisionId,
    RuntimeCatalogIngestBatch, RuntimeChangedRecord, RuntimeDocFacetRecord,
    RuntimeEdgeAuthorityRecord, RuntimeMetadataQueryRequest, RuntimeSnapshotRecord,
    SearchCorpusIngestBatch, SearchCorpusReplaceScope, SearchCorpusTombstoneScope,
    SearchPlaneIngestIpcRequest, SearchPlaneIngestIpcRequestEnvelope, SearchPlaneIngestIpcResponse,
    SearchPlaneIngestIpcResponseEnvelope, SearchPlaneQueryIpcRequest,
    SearchPlaneQueryIpcRequestEnvelope, SearchPlaneQueryIpcResponse,
    SearchPlaneQueryIpcResponseEnvelope, SearchScopeKey, SearchScopeSurface, SemanticQueryRequest,
    StructuralIngestBatch, StructuralQueryRequest, StructuralReplaceScope,
    StructuralTombstoneScope, StructuralTreeRecord, TextQueryRequest, TextQuerySyntax,
};
use quanta_index_ipc::send_request;
use quanta_index_searchd::app::config::OpenAiEmbedderTuning;
use quanta_index_searchd::app::searchd::drive;
use quanta_index_searchd::app::{SearchdConfig, SemanticEmbedderProfile};
use quanta_index_searchd_runtime::build_runtime;
use serde::ser::{Serialize, SerializeStruct, Serializer};

use crate::frontdoor_scenarios::{
    IPC_FRONTDOOR_SCENARIOS, IpcFrontdoorExpectation, IpcFrontdoorSurface,
};

type TestResult = Result<(), Box<dyn Error>>;
static NEXT_SOCKET_ID: AtomicU64 = AtomicU64::new(0);
const READINESS_TIMEOUT: Duration = Duration::from_secs(15);
const SOCKET_APPEAR_TIMEOUT: Duration = Duration::from_secs(5);
type RuntimeHandles = (
    std::path::PathBuf,
    std::path::PathBuf,
    Arc<AtomicBool>,
    DriverJoin,
);

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
    RepoId::new("repo-int")
}

fn revision() -> RevisionId {
    RevisionId::new("rev-int")
}

fn generation() -> ManifestGeneration {
    ManifestGeneration::new(7)
}

fn chunk_payload(text: &str) -> Result<Vec<u8>, Box<dyn Error>> {
    chunk_payload_with_metadata("", "", 0, 0, text)
}

fn chunk_record(id: &str, text: &str) -> Result<ChunkRecord, Box<dyn Error>> {
    chunk_record_with_metadata(id, "src/e2e.txt", "text", 0, 0, text)
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
        source_repo_id: None,
    })
}

fn chunk_payload_with_metadata(
    repo_relative_path: &str,
    language: &str,
    start_line: u32,
    end_line: u32,
    text: &str,
) -> Result<Vec<u8>, Box<dyn Error>> {
    let repo_relative_path = if repo_relative_path.is_empty() {
        "src/e2e.txt"
    } else {
        repo_relative_path
    };
    let language = if language.is_empty() {
        "text"
    } else {
        language
    };
    let record = ChunkRecord {
        chunk_id: ChunkId::new("payload-chunk"),
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
        source_repo_id: None,
    };
    let mut buf = Vec::new();
    ciborium::into_writer(&record, &mut buf)
        .map_err(|err| -> Box<dyn Error> { format!("encode chunk: {err}").into() })?;
    Ok(buf)
}

fn encode_cbor<T: Serialize>(value: &T, label: &str) -> Result<Vec<u8>, Box<dyn Error>> {
    let mut buf = Vec::new();
    ciborium::into_writer(value, &mut buf)
        .map_err(|err| -> Box<dyn Error> { format!("encode {label}: {err}").into() })?;
    Ok(buf)
}

fn history_commit_sha() -> CommitSha {
    CommitSha::from_bytes([
        0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd,
        0xef, 0x01, 0x23, 0x45, 0x67,
    ])
}

fn history_commit_record() -> CommitRecord {
    CommitRecord {
        wire_version: 1,
        sha: history_commit_sha(),
        parents: Vec::new(),
        author_time_ms: 11,
        committer_time_ms: 12,
        applied_at_ms: 13,
        author: "alice".to_string().into_boxed_str(),
        author_name: None,
        author_email: None,
        committer: "alice".to_string().into_boxed_str(),
        committer_name: None,
        committer_email: None,
        message: "fix: sample".to_string().into_boxed_str(),
        is_merge: false,
        tags: vec!["v1.0.0".to_string().into_boxed_str()],
    }
}

fn structural_role_tags(
    root_end: u32,
    identifier_start: u32,
    identifier_end: u32,
    block_start: u32,
    block_end: u32,
) -> Vec<ParseRoleTag> {
    vec![
        ParseRoleTag {
            role: "item".to_string().into_boxed_str(),
            byte_start: 0,
            byte_end: root_end,
        },
        ParseRoleTag {
            role: "expr".to_string().into_boxed_str(),
            byte_start: identifier_start,
            byte_end: identifier_end,
        },
        ParseRoleTag {
            role: "stmt".to_string().into_boxed_str(),
            byte_start: block_start,
            byte_end: block_end,
        },
    ]
}

fn structural_tree_record() -> Result<ParseTreeRecord, Box<dyn Error>> {
    Ok(ParseTreeRecord {
        wire_version: 1,
        lang: LanguageCode::new("rust")
            .map_err(|err| -> Box<dyn Error> { format!("invalid tree lang: {err}").into() })?,
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
        role_tags: structural_role_tags(10, 3, 7, 8, 10),
    })
}

fn publish_structural_ready_fixture(socket: &Path) -> TestResult {
    publish_search_corpus_chunks(
        socket,
        vec![chunk_record_with_metadata(
            "chunk-tree",
            "src/lib.rs",
            "rust",
            1,
            1,
            "fn main() {}",
        )?],
        Some(b"manifest".to_vec()),
    )?;
    publish_structural_scope(
        socket,
        "src/lib.rs",
        vec![StructuralTreeRecord {
            chunk_id: ChunkId::new("chunk-tree"),
            record: structural_tree_record()?,
        }],
    )
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

fn unique_socket_paths() -> (std::path::PathBuf, std::path::PathBuf) {
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let sequence = NEXT_SOCKET_ID.fetch_add(1, Ordering::Relaxed);
    let query = std::env::temp_dir().join(format!("qi-query-test-{pid}-{nanos}-{sequence}.sock"));
    let control =
        std::env::temp_dir().join(format!("qi-control-test-{pid}-{nanos}-{sequence}.sock"));
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
    // The unit socket path under tmpdir state root can exceed the 104-byte
    // AF_UNIX limit on macOS for long temp paths; use a flat path in
    // /tmp instead.
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

fn send_ingest_request(
    socket: &Path,
    request: &SearchPlaneIngestIpcRequestEnvelope,
) -> Result<SearchPlaneIngestIpcResponseEnvelope, quanta_index_ipc::IpcError> {
    send_request(socket, request, quanta_index_ipc::ClientIoPolicy::default())
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

type DriverJoin = thread::JoinHandle<anyhow::Result<()>>;

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

/// Hermetic semantic smoke: force the deterministic hash embedder explicitly so the
/// real daemon/query path is proven without ambient env drift or live-network deps.
fn start_runtime_with_hash(
    state_root: &Path,
    thread_name: &str,
) -> Result<RuntimeHandles, Box<dyn Error>> {
    let mut config = SearchdConfig::from_state_root(state_root.to_path_buf())
        .try_with_search_corpus_history_retention_limits(
            8,
            16 * 1024 * 1024,
            128,
            256 * 1024 * 1024,
        )
        .expect("valid test retention policy");
    let (query_socket, control_socket) = unique_socket_paths();
    config = SearchdConfig::with_socket_overrides(config, query_socket, control_socket);
    config = config.with_semantic_embedder_profile(SemanticEmbedderProfile::Hash {
        dimension: quanta_index_search_plane::SEARCH_OWNED_SEMANTIC_DIMENSION,
    });
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
        return Err("hash semantic smoke: sockets never appeared".into());
    }
    Ok((query_socket, ingest_socket, shutdown, join))
}

fn stop_runtime(shutdown: Arc<AtomicBool>, join: DriverJoin) -> TestResult {
    shutdown.store(true, Ordering::Release);
    drop(shutdown);
    match join.join() {
        Ok(Ok(())) => Ok(()),
        Ok(Err(err)) => Err(err.into()),
        Err(panic) => Err(format!("driver panic: {panic:?}").into()),
    }
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
        SearchPlaneIngestIpcResponse::Error(err) => {
            Err(format!("ingest failed code={} message={}", err.code, err.message).into())
        }
        _ => Ok(()),
    }
}

fn publish_search_corpus_chunks(
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
        .map(|(path, chunks)| SearchCorpusReplaceScope {
            scope: scope_key(&path),
            scope_digest: format!("e2e-lex-scope:{path}"),
            chunks,
            symbols: Vec::new(),
        })
        .collect();
    dispatch_ingest(
        socket,
        SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(SearchCorpusIngestBatch {
            repo_id: repo(),
            revision_id: revision(),
            generation: generation(),
            base_generation: None,
            manifest_digest: format!("e2e-lex-manifest-{}", generation().get()),
            batch_digest: format!(
                "e2e-lex-batch-{}",
                NEXT_SOCKET_ID.fetch_add(1, Ordering::Relaxed)
            ),
            mode: BatchIngestMode::ReplaceGeneration,
            bundle_payload,
            clear_surfaces: Vec::new(),
            replace_scopes,
            tombstone_scopes: Vec::new(),
            semantic_replace_scopes: Vec::new(),
            semantic_tombstone_scopes: Vec::new(),
            seal: false,
        }),
    )
}

fn tombstone_lexical_scopes(socket: &Path, paths: &[&str]) -> TestResult {
    dispatch_ingest(
        socket,
        SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(SearchCorpusIngestBatch {
            repo_id: repo(),
            revision_id: revision(),
            generation: generation(),
            base_generation: None,
            manifest_digest: format!("e2e-lex-del-{}", generation().get()),
            batch_digest: format!(
                "e2e-lex-del-batch-{}",
                NEXT_SOCKET_ID.fetch_add(1, Ordering::Relaxed)
            ),
            mode: BatchIngestMode::ReplaceGeneration,
            bundle_payload: None,
            clear_surfaces: Vec::new(),
            replace_scopes: Vec::new(),
            tombstone_scopes: paths
                .iter()
                .map(|path| SearchCorpusTombstoneScope {
                    scope: scope_key(path),
                })
                .collect(),
            semantic_replace_scopes: Vec::new(),
            semantic_tombstone_scopes: Vec::new(),
            seal: false,
        }),
    )
}

fn seal_lexical(socket: &Path) -> TestResult {
    dispatch_ingest(
        socket,
        SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(SearchCorpusIngestBatch {
            repo_id: repo(),
            revision_id: revision(),
            generation: generation(),
            base_generation: None,
            manifest_digest: format!("e2e-lex-seal-{}", generation().get()),
            batch_digest: format!(
                "e2e-lex-seal-batch-{}",
                NEXT_SOCKET_ID.fetch_add(1, Ordering::Relaxed)
            ),
            mode: BatchIngestMode::ReplaceGeneration,
            bundle_payload: None,
            clear_surfaces: Vec::new(),
            replace_scopes: Vec::new(),
            tombstone_scopes: Vec::new(),
            semantic_replace_scopes: Vec::new(),
            semantic_tombstone_scopes: Vec::new(),
            seal: true,
        }),
    )
}

fn publish_history_commits(socket: &Path, commits: Vec<CommitRecord>) -> TestResult {
    dispatch_ingest(
        socket,
        SearchPlaneIngestIpcRequest::PublishHistoryBatch(HistoryIngestBatch {
            repo_id: repo(),
            revision_id: revision(),
            generation: generation(),
            manifest_digest: Some(format!("e2e-history-manifest-{}", generation().get())),
            batch_digest: format!(
                "e2e-history-batch-{}",
                NEXT_SOCKET_ID.fetch_add(1, Ordering::Relaxed)
            ),
            commits,
            refs: Vec::new(),
            tags: Vec::new(),
            diff_hunks: Vec::new(),
        }),
    )
}

fn publish_history_authority_fixture(socket: &Path) -> TestResult {
    use quanta_index_contract::lex::DiffHunkRecord;
    use quanta_index_contract::{
        DiffHunkSide, HistoryDiffHunkUpsert, HistoryRefMutation, HistoryRefUpsert,
    };

    let commit_sha = history_commit_sha();
    dispatch_ingest(
        socket,
        SearchPlaneIngestIpcRequest::PublishHistoryBatch(HistoryIngestBatch {
            repo_id: repo(),
            revision_id: revision(),
            generation: generation(),
            manifest_digest: Some(format!("e2e-history-authority-{}", generation().get())),
            batch_digest: format!(
                "e2e-history-authority-batch-{}",
                NEXT_SOCKET_ID.fetch_add(1, Ordering::Relaxed)
            ),
            commits: vec![CommitRecord {
                wire_version: 1,
                sha: commit_sha,
                parents: Vec::new(),
                author_time_ms: 11,
                committer_time_ms: 12,
                applied_at_ms: 13,
                author: "alice".to_string().into_boxed_str(),
                author_name: None,
                author_email: None,
                committer: "alice".to_string().into_boxed_str(),
                committer_name: None,
                committer_email: None,
                message: "fix: sample alpha_content_needle"
                    .to_string()
                    .into_boxed_str(),
                is_merge: false,
                tags: vec!["v1.0.0".to_string().into_boxed_str()],
            }],
            refs: vec![HistoryRefMutation::Upsert(HistoryRefUpsert {
                name: "refs/heads/main".to_string().into_boxed_str(),
                sha: commit_sha,
            })],
            tags: vec![HistoryRefMutation::Upsert(HistoryRefUpsert {
                name: "v1.0.0".to_string().into_boxed_str(),
                sha: commit_sha,
            })],
            diff_hunks: vec![HistoryDiffHunkUpsert {
                commit_sha,
                file_path: "src/history.rs".to_string().into_boxed_str(),
                record: DiffHunkRecord {
                    wire_version: 1,
                    hunk_header: "@@ -1 +1 @@".to_string().into_boxed_str(),
                    side: DiffHunkSide::After,
                    added_text: "history added token".to_string().into_boxed_str(),
                    removed_text: "history removed token".to_string().into_boxed_str(),
                    touched_text: "history touched token".to_string().into_boxed_str(),
                    byte_start: 0,
                    byte_end: 20,
                },
            }],
        }),
    )
}

fn publish_runtime_catalog_fixture(socket: &Path) -> TestResult {
    dispatch_ingest(
        socket,
        SearchPlaneIngestIpcRequest::PublishRuntimeCatalogBatch(RuntimeCatalogIngestBatch {
            repo_id: repo(),
            revision_id: revision(),
            generation: generation(),
            overlay_epoch_ms: 20,
            batch_digest: format!(
                "e2e-runtime-catalog-batch-{}",
                NEXT_SOCKET_ID.fetch_add(1, Ordering::Relaxed)
            ),
            producer_head_applied_at_ms: 100,
            generation_materialized_at_ms: 20,
            changed_entries: vec![RuntimeChangedRecord {
                doc_id: ChunkId::new("changed"),
                applied_at_ms: 25,
                payload_hash: [0xaa; 32],
            }],
            facet_entries: vec![
                RuntimeDocFacetRecord {
                    doc_id: ChunkId::new("owner"),
                    owner: Some("team-a".to_string()),
                    service: Some("search".to_string()),
                    layer: Some("index".to_string()),
                    surface: Some("lexical".to_string()),
                },
                RuntimeDocFacetRecord {
                    doc_id: ChunkId::new("owner-other"),
                    owner: Some("team-b".to_string()),
                    service: Some("search".to_string()),
                    layer: Some("index".to_string()),
                    surface: Some("lexical".to_string()),
                },
                RuntimeDocFacetRecord {
                    doc_id: ChunkId::new("service"),
                    owner: Some("team-a".to_string()),
                    service: Some("search".to_string()),
                    layer: Some("index".to_string()),
                    surface: Some("lexical".to_string()),
                },
                RuntimeDocFacetRecord {
                    doc_id: ChunkId::new("service-other"),
                    owner: Some("team-a".to_string()),
                    service: Some("build".to_string()),
                    layer: Some("index".to_string()),
                    surface: Some("lexical".to_string()),
                },
                RuntimeDocFacetRecord {
                    doc_id: ChunkId::new("layer"),
                    owner: Some("team-a".to_string()),
                    service: Some("search".to_string()),
                    layer: Some("index".to_string()),
                    surface: Some("lexical".to_string()),
                },
                RuntimeDocFacetRecord {
                    doc_id: ChunkId::new("layer-other"),
                    owner: Some("team-a".to_string()),
                    service: Some("search".to_string()),
                    layer: Some("query".to_string()),
                    surface: Some("lexical".to_string()),
                },
                RuntimeDocFacetRecord {
                    doc_id: ChunkId::new("surface"),
                    owner: Some("team-a".to_string()),
                    service: Some("search".to_string()),
                    layer: Some("index".to_string()),
                    surface: Some("lexical".to_string()),
                },
                RuntimeDocFacetRecord {
                    doc_id: ChunkId::new("surface-other"),
                    owner: Some("team-a".to_string()),
                    service: Some("search".to_string()),
                    layer: Some("index".to_string()),
                    surface: Some("semantic".to_string()),
                },
            ],
            snapshot_entries: vec![RuntimeSnapshotRecord {
                name: "active".to_string(),
                doc_ids: vec![ChunkId::new("changed"), ChunkId::new("snap")],
            }],
            affected_entries: vec![RuntimeEdgeAuthorityRecord {
                key: "rebuild=lexical".to_string(),
                doc_ids: vec![ChunkId::new("changed")],
            }],
            invalidated_by_entries: vec![RuntimeEdgeAuthorityRecord {
                key: "rebuild=lexical".to_string(),
                doc_ids: vec![ChunkId::new("changed")],
            }],
        }),
    )
}

fn publish_structural_scope(
    socket: &Path,
    path: &str,
    trees: Vec<StructuralTreeRecord>,
) -> TestResult {
    dispatch_ingest(
        socket,
        SearchPlaneIngestIpcRequest::PublishStructuralBatch(StructuralIngestBatch {
            repo_id: repo(),
            revision_id: revision(),
            generation: generation(),
            base_generation: None,
            manifest_digest: format!("e2e-struct-manifest-{}", generation().get()),
            batch_digest: format!(
                "e2e-struct-batch-{}",
                NEXT_SOCKET_ID.fetch_add(1, Ordering::Relaxed)
            ),
            mode: BatchIngestMode::ReplaceGeneration,
            replace_scopes: vec![StructuralReplaceScope {
                scope: scope_key(path),
                scope_digest: format!("e2e-struct-scope:{path}"),
                trees,
            }],
            tombstone_scopes: Vec::new(),
            seal: false,
        }),
    )
}

fn tombstone_structural_scopes(socket: &Path, paths: &[&str]) -> TestResult {
    dispatch_ingest(
        socket,
        SearchPlaneIngestIpcRequest::PublishStructuralBatch(StructuralIngestBatch {
            repo_id: repo(),
            revision_id: revision(),
            generation: generation(),
            base_generation: None,
            manifest_digest: format!("e2e-struct-del-{}", generation().get()),
            batch_digest: format!(
                "e2e-struct-del-batch-{}",
                NEXT_SOCKET_ID.fetch_add(1, Ordering::Relaxed)
            ),
            mode: BatchIngestMode::ReplaceGeneration,
            replace_scopes: Vec::new(),
            tombstone_scopes: paths
                .iter()
                .map(|path| StructuralTombstoneScope {
                    scope: scope_key(path),
                })
                .collect(),
            seal: false,
        }),
    )
}

fn seal_structural(socket: &Path) -> TestResult {
    dispatch_ingest(
        socket,
        SearchPlaneIngestIpcRequest::PublishStructuralBatch(StructuralIngestBatch {
            repo_id: repo(),
            revision_id: revision(),
            generation: generation(),
            base_generation: None,
            manifest_digest: format!("e2e-struct-seal-{}", generation().get()),
            batch_digest: format!(
                "e2e-struct-seal-batch-{}",
                NEXT_SOCKET_ID.fetch_add(1, Ordering::Relaxed)
            ),
            mode: BatchIngestMode::ReplaceGeneration,
            replace_scopes: Vec::new(),
            tombstone_scopes: Vec::new(),
            seal: true,
        }),
    )
}

#[test]
fn publish_dispatch_query_lexical_roundtrip() -> TestResult {
    let dir = tempfile::tempdir()?;
    let state_root = dir.path();
    let (socket, ingest_socket, shutdown, join) = start_runtime(state_root, "searchd-test-driver")?;
    publish_search_corpus_chunks(
        &ingest_socket,
        vec![
            chunk_record("c1", "hello world")?,
            chunk_record("c2", "hello rust")?,
            chunk_record("c3", "goodbye")?,
        ],
        Some(b"manifest".to_vec()),
    )?;
    seal_lexical(&ingest_socket)?;
    let mut ready_candidates = None;
    if !wait_until(READINESS_TIMEOUT, || {
        let probe = lex_query("hello");
        match send_query_request(&socket, &probe) {
            Ok(resp) => match resp.payload {
                SearchPlaneQueryIpcResponse::Text(lex) => {
                    if lex.results.len() == 2 {
                        ready_candidates = Some(lex.results);
                        true
                    } else {
                        false
                    }
                }
                SearchPlaneQueryIpcResponse::Symbol(_)
                | SearchPlaneQueryIpcResponse::Semantic(_)
                | SearchPlaneQueryIpcResponse::Hybrid(_)
                | SearchPlaneQueryIpcResponse::History(_)
                | SearchPlaneQueryIpcResponse::Structural(_)
                | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
                | SearchPlaneQueryIpcResponse::Explain(_)
                | SearchPlaneQueryIpcResponse::HybridSeed(_)
                | quanta_index_contract::SearchPlaneQueryIpcResponse::ClusterMembershipRead(_)
                | quanta_index_contract::SearchPlaneQueryIpcResponse::RuntimeMetadata(_)
                | SearchPlaneQueryIpcResponse::Error(_) => false,
            },
            Err(_) => false,
        }
    }) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("dispatcher never sealed generation".into());
    }

    let candidates = ready_candidates
        .ok_or_else(|| "dispatcher readiness probe lost lexical results".to_string())?;
    if candidates.len() != 2 {
        return Err(format!("expected 2 candidates, got {}", candidates.len()).into());
    }
    let ids: Vec<String> = candidates.iter().map(|c| c.candidate_id.clone()).collect();
    if !ids.iter().any(|i| i == "c1") || !ids.iter().any(|i| i == "c2") {
        return Err(format!("missing expected ids: {ids:?}").into());
    }

    stop_runtime(shutdown, join)
}

#[test]
fn publish_dispatch_query_sourcegraph_roundtrip() -> TestResult {
    let dir = tempfile::tempdir()?;
    let state_root = dir.path();
    let (socket, ingest_socket, shutdown, join) =
        start_runtime(state_root, "searchd-sourcegraph-query-test")?;
    publish_search_corpus_chunks(
        &ingest_socket,
        vec![
            chunk_record("c1", "hello world")?,
            chunk_record("c2", "hello rust")?,
            chunk_record("c3", "goodbye")?,
        ],
        Some(b"manifest".to_vec()),
    )?;
    seal_lexical(&ingest_socket)?;
    if !wait_until(READINESS_TIMEOUT, || {
        send_query_request(&socket, &lex_query("hello"))
            .map(|resp| matches!(resp.payload, SearchPlaneQueryIpcResponse::Text(_)))
            .unwrap_or(false)
    }) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("dispatcher never sealed sourcegraph generation".into());
    }

    let pin = GenerationPin::new(repo(), revision(), generation());
    let response = send_query_request(
        &socket,
        &SearchPlaneQueryIpcRequestEnvelope {
            request_id: 44,
            payload: SearchPlaneQueryIpcRequest::Text(TextQueryRequest {
                syntax: TextQuerySyntax::Sourcegraph,
                query_text: "hello".to_string(),
                constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                generation: Some(pin.clone()),
                generation_selector: None,
                top_k: 50,
            }),
        },
    )?;
    let sourcegraph = match response.payload {
        SearchPlaneQueryIpcResponse::Text(payload) => payload,
        other => {
            shutdown.store(true, Ordering::Release);
            drop(join.join());
            return Err(format!("expected Text, got {other:?}").into());
        }
    };
    if sourcegraph.generation != pin {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("sourcegraph response generation did not echo request pin".into());
    }
    let ids: Vec<String> = sourcegraph
        .results
        .iter()
        .map(|candidate| candidate.candidate_id.clone())
        .collect();
    if !ids.iter().any(|id| id == "c1") || !ids.iter().any(|id| id == "c2") {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!("missing expected sourcegraph ids: {ids:?}").into());
    }

    stop_runtime(shutdown, join)
}

#[test]
fn sourcegraph_path_and_lang_filters_execute_against_indexed_metadata() -> TestResult {
    let dir = tempfile::tempdir()?;
    let state_root = dir.path();
    let (socket, ingest_socket, shutdown, join) =
        start_runtime(state_root, "searchd-sourcegraph-metadata-test")?;
    publish_search_corpus_chunks(
        &ingest_socket,
        vec![
            chunk_record_with_metadata("alpha", "src/lib.rs", "rust", 3, 8, "needle alpha")?,
            chunk_record_with_metadata("beta", "src/main.rs", "rust", 10, 18, "needle beta")?,
            chunk_record_with_metadata("gamma", "src/lib.py", "python", 20, 24, "needle gamma")?,
        ],
        Some(b"manifest".to_vec()),
    )?;
    seal_lexical(&ingest_socket)?;

    let req = SearchPlaneQueryIpcRequestEnvelope {
        request_id: 7,
        payload: SearchPlaneQueryIpcRequest::Text(TextQueryRequest {
            syntax: TextQuerySyntax::Sourcegraph,
            query_text: "path:src/lib.rs lang:rust needle".to_string(),
            constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
            generation: Some(GenerationPin::new(repo(), revision(), generation())),
            generation_selector: None,
            top_k: 50,
        }),
    };

    if !wait_until(READINESS_TIMEOUT, || {
        send_query_request(&socket, &req)
            .map(|resp| !matches!(resp.payload, SearchPlaneQueryIpcResponse::Error(_)))
            .unwrap_or(false)
    }) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("sourcegraph metadata query never became ready".into());
    }

    let response = send_query_request(&socket, &req)?;
    let results = match response.payload {
        SearchPlaneQueryIpcResponse::Text(lexical) => lexical.results,
        other => {
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
    if candidate.start_line != 3 || candidate.end_line != 8 {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!(
            "expected line span 3..8, got {}..{}",
            candidate.start_line, candidate.end_line
        )
        .into());
    }

    stop_runtime(shutdown, join)
}

#[test]
fn history_query_returns_typed_generation_not_ready_error() -> TestResult {
    let dir = tempfile::tempdir()?;
    let state_root = dir.path();
    let (socket, _ingest_socket, shutdown, join) =
        start_runtime(state_root, "searchd-history-generation-not-ready-test")?;

    let err = wait_for_typed_error(
        &socket,
        &history_query("type:commit fix"),
        READINESS_TIMEOUT,
    )?;
    if err.code != "HISTORY_GENERATION_NOT_READY" {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!("expected HISTORY_GENERATION_NOT_READY, got {}", err.code).into());
    }
    if !err.message.contains("not yet materialized") {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!("unexpected generation-not-ready message: {}", err.message).into());
    }

    stop_runtime(shutdown, join)
}

#[test]
fn history_query_returns_typed_producer_unavailable_without_lexical_fallback() -> TestResult {
    let dir = tempfile::tempdir()?;
    let state_root = dir.path();
    let (socket, ingest_socket, shutdown, join) =
        start_runtime(state_root, "searchd-history-producer-unavailable-test")?;
    publish_search_corpus_chunks(
        &ingest_socket,
        vec![chunk_record(
            "history-fallback",
            "fix only lives in lexical content",
        )?],
        Some(b"manifest".to_vec()),
    )?;
    seal_lexical(&ingest_socket)?;
    if !wait_until(READINESS_TIMEOUT, || {
        send_query_request(&socket, &lex_query("fix"))
            .map(|resp| matches!(resp.payload, SearchPlaneQueryIpcResponse::Text(_)))
            .unwrap_or(false)
    }) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("lexical fixture never became queryable".into());
    }

    let err = wait_for_typed_error(
        &socket,
        &history_query("type:commit fix"),
        READINESS_TIMEOUT,
    )?;
    if err.code != "HISTORY_PRODUCER_UNAVAILABLE" {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!("expected HISTORY_PRODUCER_UNAVAILABLE, got {}", err.code).into());
    }
    if !err.message.contains("producer data is unavailable") {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!("unexpected producer-unavailable message: {}", err.message).into());
    }

    stop_runtime(shutdown, join)
}

#[test]
fn history_query_returns_typed_shard_unavailable_when_diff_shard_missing() -> TestResult {
    let dir = tempfile::tempdir()?;
    let state_root = dir.path();
    let (socket, ingest_socket, shutdown, join) =
        start_runtime(state_root, "searchd-history-shard-unavailable-test")?;
    publish_search_corpus_chunks(
        &ingest_socket,
        vec![chunk_record("history-lex", "history shard lexical proof")?],
        Some(b"manifest".to_vec()),
    )?;
    publish_history_commits(&ingest_socket, vec![history_commit_record()])?;
    seal_lexical(&ingest_socket)?;
    if !wait_until(READINESS_TIMEOUT, || {
        send_query_request(&socket, &lex_query("history"))
            .map(|resp| matches!(resp.payload, SearchPlaneQueryIpcResponse::Text(_)))
            .unwrap_or(false)
    }) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("history lexical proof never became ready".into());
    }

    let err = wait_for_typed_error(
        &socket,
        &history_query("type:diff history"),
        READINESS_TIMEOUT,
    )?;
    if err.code != "HISTORY_SHARD_UNAVAILABLE" {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!("expected HISTORY_SHARD_UNAVAILABLE, got {}", err.code).into());
    }
    if !err.message.contains("diff shard is unavailable") {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!("unexpected shard-unavailable message: {}", err.message).into());
    }

    stop_runtime(shutdown, join)
}

#[test]
fn end_to_end_widened_history_and_runtime_queries_roundtrip_exact_truth() -> TestResult {
    let dir = tempfile::tempdir()?;
    let state_root = dir.path();
    let (socket, ingest_socket, shutdown, join) =
        start_runtime(state_root, "searchd-frontdoor-history-runtime-matrix")?;
    publish_search_corpus_chunks(
        &ingest_socket,
        vec![
            chunk_record("history-lex", "history lexical proof")?,
            chunk_record_with_metadata(
                "changed",
                "src/changed.rs",
                "rust",
                1,
                1,
                "catalog_changed_needle",
            )?,
            chunk_record_with_metadata(
                "snap",
                "src/snap.rs",
                "rust",
                1,
                1,
                "catalog_snapshot_needle",
            )?,
            chunk_record_with_metadata(
                "snap-other",
                "src/snap-other.rs",
                "rust",
                1,
                1,
                "catalog_snapshot_needle",
            )?,
            chunk_record_with_metadata(
                "owner",
                "src/owner.rs",
                "rust",
                1,
                1,
                "catalog_owner_needle",
            )?,
            chunk_record_with_metadata(
                "owner-other",
                "src/owner-other.rs",
                "rust",
                1,
                1,
                "catalog_owner_needle",
            )?,
            chunk_record_with_metadata(
                "service",
                "src/service.rs",
                "rust",
                1,
                1,
                "catalog_service_needle",
            )?,
            chunk_record_with_metadata(
                "service-other",
                "src/service-other.rs",
                "rust",
                1,
                1,
                "catalog_service_needle",
            )?,
            chunk_record_with_metadata(
                "layer",
                "src/layer.rs",
                "rust",
                1,
                1,
                "catalog_layer_needle",
            )?,
            chunk_record_with_metadata(
                "layer-other",
                "src/layer-other.rs",
                "rust",
                1,
                1,
                "catalog_layer_needle",
            )?,
            chunk_record_with_metadata(
                "surface",
                "src/surface.rs",
                "rust",
                1,
                1,
                "catalog_surface_needle",
            )?,
            chunk_record_with_metadata(
                "surface-other",
                "src/surface-other.rs",
                "rust",
                1,
                1,
                "catalog_surface_needle",
            )?,
        ],
        Some(b"manifest".to_vec()),
    )?;
    publish_history_authority_fixture(&ingest_socket)?;
    publish_runtime_catalog_fixture(&ingest_socket)?;
    seal_lexical(&ingest_socket)?;

    for &scenario in IPC_FRONTDOOR_SCENARIOS {
        match scenario.expected {
            IpcFrontdoorExpectation::TypedError(expected_error) => {
                let request = match scenario.surface {
                    IpcFrontdoorSurface::History => {
                        history_query_with_syntax(scenario.syntax, scenario.query_text)
                    }
                    IpcFrontdoorSurface::RuntimeMetadata => {
                        runtime_metadata_query_with_syntax(scenario.syntax, scenario.query_text)
                    }
                };
                let err = wait_for_typed_error(&socket, &request, READINESS_TIMEOUT)?;
                if err.code != expected_error.code
                    || !err.message.contains(expected_error.message_contains)
                {
                    shutdown.store(true, Ordering::Release);
                    drop(join.join());
                    return Err(format!(
                        "{} typed error drifted: expected code={} fragment={:?}, got code={} message={}",
                        scenario.name,
                        expected_error.code,
                        expected_error.message_contains,
                        err.code,
                        err.message
                    )
                    .into());
                }
            }
            IpcFrontdoorExpectation::CommitShas(expected_shas) => {
                let request = history_query_with_syntax(scenario.syntax, scenario.query_text);
                if !wait_until(READINESS_TIMEOUT, || {
                    send_query_request(&socket, &request)
                        .map(|resp| match resp.payload {
                            SearchPlaneQueryIpcResponse::History(_) => true,
                            SearchPlaneQueryIpcResponse::Error(err) => err.code != "NOT_READY",
                            _ => false,
                        })
                        .unwrap_or(false)
                }) {
                    shutdown.store(true, Ordering::Release);
                    drop(join.join());
                    return Err(format!("{} never became ready", scenario.name).into());
                }
                let response = send_query_request(&socket, &request)?;
                let history = match response.payload {
                    SearchPlaneQueryIpcResponse::History(history) => history,
                    other => {
                        shutdown.store(true, Ordering::Release);
                        drop(join.join());
                        return Err(
                            format!("{} expected History, got {other:?}", scenario.name).into()
                        );
                    }
                };
                let observed = history
                    .commits
                    .iter()
                    .map(|commit| commit.sha.to_hex())
                    .collect::<Vec<_>>();
                let expected = expected_shas
                    .iter()
                    .map(|sha| (*sha).to_string())
                    .collect::<Vec<_>>();
                if observed != expected || !history.diffs.is_empty() {
                    shutdown.store(true, Ordering::Release);
                    drop(join.join());
                    return Err(format!(
                        "{} commit drifted: expected {:?}, got commits={:?} diffs={:?}",
                        scenario.name, expected, observed, history.diffs
                    )
                    .into());
                }
            }
            IpcFrontdoorExpectation::DiffPaths(expected_paths) => {
                let request = history_query_with_syntax(scenario.syntax, scenario.query_text);
                if !wait_until(READINESS_TIMEOUT, || {
                    send_query_request(&socket, &request)
                        .map(|resp| match resp.payload {
                            SearchPlaneQueryIpcResponse::History(_) => true,
                            SearchPlaneQueryIpcResponse::Error(err) => err.code != "NOT_READY",
                            _ => false,
                        })
                        .unwrap_or(false)
                }) {
                    shutdown.store(true, Ordering::Release);
                    drop(join.join());
                    return Err(format!("{} never became ready", scenario.name).into());
                }
                let response = send_query_request(&socket, &request)?;
                let history = match response.payload {
                    SearchPlaneQueryIpcResponse::History(history) => history,
                    other => {
                        shutdown.store(true, Ordering::Release);
                        drop(join.join());
                        return Err(
                            format!("{} expected History, got {other:?}", scenario.name).into()
                        );
                    }
                };
                let observed = history
                    .diffs
                    .iter()
                    .map(|diff| diff.repo_relative_path.clone())
                    .collect::<Vec<_>>();
                let expected = expected_paths
                    .iter()
                    .map(|path| (*path).to_string())
                    .collect::<Vec<_>>();
                if observed != expected || !history.commits.is_empty() {
                    shutdown.store(true, Ordering::Release);
                    drop(join.join());
                    return Err(format!(
                        "{} diff drifted: expected {:?}, got diffs={:?} commits={:?}",
                        scenario.name, expected, observed, history.commits
                    )
                    .into());
                }
            }
            IpcFrontdoorExpectation::CandidateIds(expected_ids) => {
                let request =
                    runtime_metadata_query_with_syntax(scenario.syntax, scenario.query_text);
                if !wait_until(READINESS_TIMEOUT, || {
                    send_query_request(&socket, &request)
                        .map(|resp| match resp.payload {
                            quanta_index_contract::SearchPlaneQueryIpcResponse::RuntimeMetadata(
                                _,
                            ) => true,
                            SearchPlaneQueryIpcResponse::Error(err) => err.code != "NOT_READY",
                            _ => false,
                        })
                        .unwrap_or(false)
                }) {
                    shutdown.store(true, Ordering::Release);
                    drop(join.join());
                    return Err(format!("{} never became ready", scenario.name).into());
                }
                let response = send_query_request(&socket, &request)?;
                let runtime = match response.payload {
                    quanta_index_contract::SearchPlaneQueryIpcResponse::RuntimeMetadata(
                        runtime,
                    ) => runtime,
                    other => {
                        shutdown.store(true, Ordering::Release);
                        drop(join.join());
                        return Err(format!(
                            "{} expected RuntimeMetadata, got {other:?}",
                            scenario.name
                        )
                        .into());
                    }
                };
                let observed = runtime
                    .results
                    .iter()
                    .map(|candidate| candidate.candidate_id.clone())
                    .collect::<Vec<_>>();
                let expected = expected_ids
                    .iter()
                    .map(|id| (*id).to_string())
                    .collect::<Vec<_>>();
                if observed != expected {
                    shutdown.store(true, Ordering::Release);
                    drop(join.join());
                    return Err(format!(
                        "{} runtime candidate drifted: expected {:?}, got {:?}",
                        scenario.name, expected, observed
                    )
                    .into());
                }
            }
        }
    }

    stop_runtime(shutdown, join)
}

#[test]
fn hybrid_query_requires_joint_materialization() -> TestResult {
    let dir = tempfile::tempdir()?;
    let state_root = dir.path();
    let (socket, ingest_socket, shutdown, join) = start_runtime(state_root, "searchd-test-driver")?;
    drop(ingest_socket);

    // Hybrid query still fails on the lexical gate first when neither lexical
    // nor semantic generation has materialized yet.
    let hybrid_req = SearchPlaneQueryIpcRequestEnvelope {
        request_id: 1,
        payload: SearchPlaneQueryIpcRequest::Hybrid(HybridQueryRequest {
            text_query: TextQueryRequest {
                syntax: TextQuerySyntax::Sourcegraph,
                query_text: "only".to_string(),
                constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                generation: Some(GenerationPin::new(repo(), revision(), generation())),
                generation_selector: None,
                top_k: 50,
            },
            semantic_query_text: "only".to_string(),
            generation: Some(GenerationPin::new(repo(), revision(), generation())),
            generation_selector: None,
            top_k: 5,
        }),
    };
    let response = send_query_request(&socket, &hybrid_req)?;
    let err = match response.payload {
        SearchPlaneQueryIpcResponse::Error(err) => err,
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

    stop_runtime(shutdown, join)
}

#[test]
fn hybrid_query_succeeds_when_both_tracks_sealed() -> TestResult {
    let dir = tempfile::tempdir()?;
    let state_root = dir.path();

    let (socket, ingest_socket, shutdown, join) = start_runtime(state_root, "searchd-test-driver")?;
    let alpha = chunk_record("alpha", "sphinx quartz")?;
    let beta = chunk_record("beta", "sphinx riddles")?;
    publish_search_corpus_chunks(&ingest_socket, vec![alpha, beta], None)?;
    seal_lexical(&ingest_socket)?;
    let pin = GenerationPin::new(repo(), revision(), generation());
    // Wait for joint lexical/semantic materialization from search-corpus ingest.
    if !wait_until(READINESS_TIMEOUT, || {
        let req = SearchPlaneQueryIpcRequestEnvelope {
            request_id: 0,
            payload: SearchPlaneQueryIpcRequest::Hybrid(HybridQueryRequest {
                text_query: TextQueryRequest {
                    syntax: TextQuerySyntax::Sourcegraph,
                    query_text: "sphinx".to_string(),
                    constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                    generation: Some(pin.clone()),
                    generation_selector: None,
                    top_k: 50,
                },
                semantic_query_text: "quartz".to_string(),
                generation: Some(pin.clone()),
                generation_selector: None,
                top_k: 5,
            }),
        };
        send_query_request(&socket, &req)
            .map(|r| !matches!(r.payload, SearchPlaneQueryIpcResponse::Error(_)))
            .unwrap_or(false)
    }) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("joint materialization never reached".into());
    }

    // Final hybrid: must return both candidates fused with alpha ranked first.
    let req = SearchPlaneQueryIpcRequestEnvelope {
        request_id: 99,
        payload: SearchPlaneQueryIpcRequest::Hybrid(HybridQueryRequest {
            text_query: TextQueryRequest {
                syntax: TextQuerySyntax::Sourcegraph,
                query_text: "sphinx".to_string(),
                constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                generation: Some(pin.clone()),
                generation_selector: None,
                top_k: 50,
            },
            semantic_query_text: "quartz".to_string(),
            generation: Some(pin),
            generation_selector: None,
            top_k: 5,
        }),
    };
    let response = send_query_request(&socket, &req)?;
    let candidates = match response.payload {
        SearchPlaneQueryIpcResponse::Hybrid(h) => h.results,
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

    stop_runtime(shutdown, join)
}

#[test]
fn hybrid_query_rejects_generation_pin_mismatch() -> TestResult {
    let dir = tempfile::tempdir()?;
    let state_root = dir.path();
    let config = build_config(state_root);
    let runtime = build_runtime(config)?;
    let socket = runtime.query_server.socket_path().to_path_buf();
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_for_drive = Arc::clone(&shutdown);
    let join = thread::Builder::new()
        .name("searchd-hybrid-pin-mismatch-test".into())
        .spawn(move || drive(runtime, &shutdown_for_drive))?;

    if !wait_until(Duration::from_secs(2), || socket.exists()) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("socket never appeared".into());
    }

    let response = send_query_request(
        &socket,
        &SearchPlaneQueryIpcRequestEnvelope {
            request_id: 40,
            payload: SearchPlaneQueryIpcRequest::Hybrid(HybridQueryRequest {
                text_query: TextQueryRequest {
                    syntax: TextQuerySyntax::Sourcegraph,
                    query_text: "needle".to_string(),
                    constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                    generation: Some(GenerationPin::new(
                        repo(),
                        revision(),
                        ManifestGeneration::new(8),
                    )),
                    generation_selector: None,
                    top_k: 50,
                },
                semantic_query_text: "needle".to_string(),
                generation: Some(GenerationPin::new(repo(), revision(), generation())),
                generation_selector: None,
                top_k: 1,
            }),
        },
    )?;
    let err = match response.payload {
        SearchPlaneQueryIpcResponse::Error(err) => err,
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
    if !err
        .message
        .contains("hybrid: lexical generation does not match semantic generation")
    {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!("unexpected mismatch message: {}", err.message).into());
    }

    shutdown.store(true, Ordering::Release);
    match join.join() {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(e.into()),
        Err(panic) => Err(format!("driver panic: {panic:?}").into()),
    }
}

#[test]
fn sourcegraph_context_filter_executes_against_repo_metadata_surface() -> TestResult {
    let dir = tempfile::tempdir()?;
    let state_root = dir.path();
    let (socket, ingest_socket, shutdown, join) =
        start_runtime(state_root, "searchd-sourcegraph-context-test")?;
    publish_search_corpus_chunks(
        &ingest_socket,
        vec![chunk_record("alpha", "needle")?],
        Some(repo_metadata_payload(
            false,
            false,
            LqVisibility::Public,
            &["global", "team-search"],
        )?),
    )?;
    seal_lexical(&ingest_socket)?;

    let req = SearchPlaneQueryIpcRequestEnvelope {
        request_id: 17,
        payload: SearchPlaneQueryIpcRequest::Text(TextQueryRequest {
            syntax: TextQuerySyntax::Sourcegraph,
            query_text: "fork:no archived:no visibility:public context:global needle".to_string(),
            constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
            generation: Some(GenerationPin::new(repo(), revision(), generation())),
            generation_selector: None,
            top_k: 50,
        }),
    };

    if !wait_until(READINESS_TIMEOUT, || {
        send_query_request(&socket, &req)
            .map(|resp| match resp.payload {
                SearchPlaneQueryIpcResponse::Error(err) => err.code != "NOT_READY",
                _ => true,
            })
            .unwrap_or(false)
    }) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("sourcegraph context filter never progressed past NOT_READY".into());
    }

    let response = send_query_request(&socket, &req)?;
    let results = match response.payload {
        SearchPlaneQueryIpcResponse::Text(text) => text.results,
        other => {
            shutdown.store(true, Ordering::Release);
            drop(join.join());
            return Err(format!("expected Text, got {other:?}").into());
        }
    };
    if results.len() != 1 {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!("expected one context-filtered hit, got {results:?}").into());
    }
    if results
        .first()
        .map(|candidate| candidate.candidate_id.as_str())
        != Some("alpha")
    {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!("expected alpha hit, got {results:?}").into());
    }

    stop_runtime(shutdown, join)
}

#[test]
fn hybrid_query_visibility_filter_executes_against_repo_metadata_surface() -> TestResult {
    let dir = tempfile::tempdir()?;
    let state_root = dir.path();
    let (socket, ingest_socket, shutdown, join) =
        start_runtime(state_root, "searchd-hybrid-lowering-error-test")?;
    let alpha = chunk_record("alpha", "needle")?;
    publish_search_corpus_chunks(
        &ingest_socket,
        vec![alpha],
        Some(repo_metadata_payload(
            false,
            false,
            LqVisibility::Public,
            &["global"],
        )?),
    )?;
    seal_lexical(&ingest_socket)?;

    let pin = GenerationPin::new(repo(), revision(), generation());
    let req = SearchPlaneQueryIpcRequestEnvelope {
        request_id: 41,
        payload: SearchPlaneQueryIpcRequest::Hybrid(HybridQueryRequest {
            text_query: TextQueryRequest {
                syntax: TextQuerySyntax::Sourcegraph,
                query_text: "fork:no archived:no visibility:public context:global needle"
                    .to_string(),
                constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                generation: Some(pin.clone()),
                generation_selector: None,
                top_k: 50,
            },
            semantic_query_text: "needle".to_string(),
            generation: Some(pin),
            generation_selector: None,
            top_k: 1,
        }),
    };

    if !wait_until(READINESS_TIMEOUT, || {
        send_query_request(&socket, &req)
            .map(|resp| match resp.payload {
                SearchPlaneQueryIpcResponse::Error(err) => err.code != "NOT_READY",
                _ => true,
            })
            .unwrap_or(false)
    }) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("hybrid visibility filter never progressed past NOT_READY".into());
    }

    let response = send_query_request(&socket, &req)?;
    let results = match response.payload {
        SearchPlaneQueryIpcResponse::Hybrid(hybrid) => hybrid.results,
        other => {
            shutdown.store(true, Ordering::Release);
            drop(join.join());
            return Err(format!("expected Hybrid, got {other:?}").into());
        }
    };
    if results.len() != 1 {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!("expected one hybrid hit, got {results:?}").into());
    }
    if results
        .first()
        .map(|candidate| candidate.candidate_id.as_str())
        != Some("alpha")
    {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!("expected alpha top hit, got {results:?}").into());
    }

    stop_runtime(shutdown, join)
}

#[test]
fn semantic_only_query_requires_semantic_materialization() -> TestResult {
    let dir = tempfile::tempdir()?;
    let state_root = dir.path();
    let config = build_config(state_root);
    let runtime = build_runtime(config)?;
    let socket = runtime.query_server.socket_path().to_path_buf();
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_for_drive = Arc::clone(&shutdown);
    let join = thread::Builder::new()
        .name("searchd-test-driver".into())
        .spawn(move || drive(runtime, &shutdown_for_drive))?;

    if !wait_until(Duration::from_secs(2), || socket.exists()) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("socket never appeared".into());
    }

    let req = SearchPlaneQueryIpcRequestEnvelope {
        request_id: 0,
        payload: SearchPlaneQueryIpcRequest::Semantic(SemanticQueryRequest {
            query_text: "semantic".to_string(),
            constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
            generation: Some(GenerationPin::new(repo(), revision(), generation())),
            generation_selector: None,
            lexical_scope: None,
            top_k: 3,
        }),
    };
    let response = send_query_request(&socket, &req)?;
    let err = match response.payload {
        SearchPlaneQueryIpcResponse::Error(e) => e,
        other => {
            shutdown.store(true, Ordering::Release);
            drop(join.join());
            return Err(format!("expected Error, got {other:?}").into());
        }
    };
    if err.code != "SEMANTIC_GENERATION_NOT_MATERIALIZED" {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!(
            "expected SEMANTIC_GENERATION_NOT_MATERIALIZED, got {}",
            err.code
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
fn semantic_query_without_lexical_scope_returns_global_nearest_hit() -> TestResult {
    let dir = tempfile::tempdir()?;
    let state_root = dir.path();
    let (socket, ingest_socket, shutdown, join) =
        start_runtime(state_root, "searchd-semantic-no-scope-success-test")?;
    let alpha = chunk_record("alpha", "semantic alpha")?;
    let beta = chunk_record("beta", "semantic beta")?;
    publish_search_corpus_chunks(&ingest_socket, vec![alpha, beta], None)?;
    seal_lexical(&ingest_socket)?;

    let pin = GenerationPin::new(repo(), revision(), generation());
    let req = SearchPlaneQueryIpcRequestEnvelope {
        request_id: 42,
        payload: SearchPlaneQueryIpcRequest::Semantic(SemanticQueryRequest {
            query_text: "alpha".to_string(),
            constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
            generation: Some(pin.clone()),
            generation_selector: None,
            lexical_scope: None,
            top_k: 1,
        }),
    };

    if !wait_until(READINESS_TIMEOUT, || {
        send_query_request(&socket, &req)
            .map(|resp| !matches!(resp.payload, SearchPlaneQueryIpcResponse::Error(_)))
            .unwrap_or(false)
    }) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("semantic no-scope query never became ready".into());
    }

    let response = send_query_request(&socket, &req)?;
    let results = match response.payload {
        SearchPlaneQueryIpcResponse::Semantic(semantic) => {
            if semantic.generation != pin {
                shutdown.store(true, Ordering::Release);
                drop(join.join());
                return Err("semantic response generation did not echo request pin".into());
            }
            semantic.results
        }
        other => {
            shutdown.store(true, Ordering::Release);
            drop(join.join());
            return Err(format!("expected Semantic, got {other:?}").into());
        }
    };
    let ids: Vec<String> = results
        .iter()
        .map(|candidate| candidate.candidate_id.clone())
        .collect();
    if ids != ["alpha".to_string()] {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!("expected global nearest [alpha], got {ids:?}").into());
    }

    stop_runtime(shutdown, join)
}

/// Boot the real daemon with an explicit `OpenAi` embedder profile.
fn start_runtime_with_openai(
    state_root: &Path,
    thread_name: &str,
    api_key: String,
) -> Result<RuntimeHandles, Box<dyn Error>> {
    let mut config = SearchdConfig::from_state_root(state_root.to_path_buf())
        .try_with_search_corpus_history_retention_limits(
            8,
            16 * 1024 * 1024,
            128,
            256 * 1024 * 1024,
        )
        .expect("valid test retention policy");
    let (query_socket, control_socket) = unique_socket_paths();
    config = SearchdConfig::with_socket_overrides(config, query_socket, control_socket);
    config = config.with_semantic_embedder_profile(SemanticEmbedderProfile::OpenAi {
        model: "text-embedding-3-small".to_string(),
        model_revision: "live".to_string(),
        dimension: 1536,
        api_key,
        tuning: OpenAiEmbedderTuning::default(),
    });
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
        return Err("openai e2e: sockets never appeared".into());
    }
    Ok((query_socket, ingest_socket, shutdown, join))
}

/// Manual release proof (gated, real `OpenAI` API).
///
/// Full daemon -> corpus embed -> lancedb cosine -> ranked results. The query
/// shares NO meaningful token with either indexed doc (only the stopword
/// "the"), so a token-distribution hash embedder (the prior FNV-1a default)
/// cannot rank them by meaning. Real neural embeddings must rank the
/// semantically-related "cat" doc above the unrelated "finance" doc. This is
/// the end-to-end capability that was structurally impossible before.
///
/// `#[ignore]` because it hits the real `OpenAI` API; run with `OPENAI_API_KEY` set:
/// `OPENAI_API_KEY=<key> cargo test -p quanta-index-searchd-runtime --test end_to_end \
///   -- --ignored openai_semantic_paraphrase_outranks_unrelated_v1 --nocapture`
#[test]
#[ignore = "hits the real OpenAI API; run with OPENAI_API_KEY set and --ignored"]
fn openai_semantic_paraphrase_outranks_unrelated_v1() -> TestResult {
    let api_key = match std::env::var("OPENAI_API_KEY") {
        Ok(key) if !key.trim().is_empty() => key,
        _ => return Err("OPENAI_API_KEY must be set to run this gated test".into()),
    };

    let dir = tempfile::tempdir()?;
    let state_root = dir.path();
    let (socket, ingest_socket, shutdown, join) =
        start_runtime_with_openai(state_root, "searchd-openai-paraphrase-e2e", api_key)?;

    // Zero meaningful lexical overlap with the query "the cat is sleeping":
    // cat-doc uses kitten/dozed/windowsill; finance-doc uses revenue/dividends.
    // Both share only the stopword "the", so lexical/hash signal is a tie —
    // only neural meaning can separate them.
    let cat_doc = chunk_record(
        "cat-doc",
        "A kitten curled up and dozed on the warm windowsill all afternoon.",
    )?;
    let finance_doc = chunk_record(
        "finance-doc",
        "Quarterly revenue and shareholder dividends climbed after the earnings report.",
    )?;
    publish_search_corpus_chunks(&ingest_socket, vec![cat_doc, finance_doc], None)?;
    seal_lexical(&ingest_socket)?;

    let pin = GenerationPin::new(repo(), revision(), generation());
    let req = SearchPlaneQueryIpcRequestEnvelope {
        request_id: 7,
        payload: SearchPlaneQueryIpcRequest::Semantic(SemanticQueryRequest {
            query_text: "the cat is sleeping".to_string(),
            constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
            generation: Some(pin.clone()),
            generation_selector: None,
            lexical_scope: None,
            top_k: 2,
        }),
    };

    if !wait_until(READINESS_TIMEOUT, || {
        send_query_request(&socket, &req)
            .map(|resp| !matches!(resp.payload, SearchPlaneQueryIpcResponse::Error(_)))
            .unwrap_or(false)
    }) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("openai semantic query never became ready".into());
    }

    let response = send_query_request(&socket, &req)?;
    let results = match response.payload {
        SearchPlaneQueryIpcResponse::Semantic(semantic) => {
            if semantic.generation != pin {
                shutdown.store(true, Ordering::Release);
                drop(join.join());
                return Err("openai semantic response did not echo request pin".into());
            }
            semantic.results
        }
        other => {
            shutdown.store(true, Ordering::Release);
            drop(join.join());
            return Err(format!("expected Semantic response, got {other:?}").into());
        }
    };

    let ranked: Vec<String> = results
        .iter()
        .map(|candidate| candidate.candidate_id.clone())
        .collect();

    // Crown assertions, evidence-backed (R-TEST-19/25): both docs retrieved, and
    // the semantically-related doc ranks strictly above the unrelated one. With
    // the hash embedder this ordering is not derivable (token-overlap tie).
    if ranked.len() != 2 {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!("expected both docs ranked, got {ranked:?}").into());
    }
    if ranked.first().map(String::as_str) != Some("cat-doc") {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!(
            "neural ranking failed: a paraphrase query should rank 'cat-doc' first; got {ranked:?}"
        )
        .into());
    }
    if ranked.get(1).map(String::as_str) != Some("finance-doc") {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(
            format!("expected unrelated 'finance-doc' ranked second, got {ranked:?}").into(),
        );
    }

    stop_runtime(shutdown, join)
}

#[test]
fn semantic_query_uses_search_owned_text_derivation_by_default() -> TestResult {
    let dir = tempfile::tempdir()?;
    let state_root = dir.path();
    let (socket, ingest_socket, shutdown, join) =
        start_runtime(state_root, "searchd-semantic-default-text-derivation-test")?;

    publish_search_corpus_chunks(
        &ingest_socket,
        vec![
            chunk_record("alpha", "parser pipeline typed semantic search")?,
            chunk_record("beta", "archive storage compaction")?,
        ],
        None,
    )?;
    seal_lexical(&ingest_socket)?;

    let req = SearchPlaneQueryIpcRequestEnvelope {
        request_id: 43,
        payload: SearchPlaneQueryIpcRequest::Semantic(SemanticQueryRequest {
            query_text: "typed semantic parser".to_string(),
            constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
            generation: Some(GenerationPin::new(repo(), revision(), generation())),
            generation_selector: None,
            lexical_scope: None,
            top_k: 1,
        }),
    };

    if !wait_until(READINESS_TIMEOUT, || {
        send_query_request(&socket, &req)
            .map(|resp| !matches!(resp.payload, SearchPlaneQueryIpcResponse::Error(_)))
            .unwrap_or(false)
    }) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("semantic default text query never became ready".into());
    }

    let response = send_query_request(&socket, &req)?;
    let semantic = match response.payload {
        SearchPlaneQueryIpcResponse::Semantic(semantic) => semantic,
        other => {
            shutdown.store(true, Ordering::Release);
            drop(join.join());
            return Err(format!("expected Semantic, got {other:?}").into());
        }
    };
    let first = semantic
        .results
        .first()
        .ok_or_else(|| "semantic default derivation returned no results".to_string())?;
    if first.candidate_id != "alpha" {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!("expected alpha candidate, got {}", first.candidate_id).into());
    }

    stop_runtime(shutdown, join)
}

#[test]
fn semantic_query_uses_search_owned_text_derivation_with_explicit_hash_profile() -> TestResult {
    let dir = tempfile::tempdir()?;
    let state_root = dir.path();
    let (socket, ingest_socket, shutdown, join) =
        start_runtime_with_hash(state_root, "searchd-semantic-explicit-hash-test")?;

    publish_search_corpus_chunks(
        &ingest_socket,
        vec![
            chunk_record("alpha", "parser pipeline typed semantic search")?,
            chunk_record("beta", "archive storage compaction")?,
        ],
        None,
    )?;
    seal_lexical(&ingest_socket)?;

    let req = SearchPlaneQueryIpcRequestEnvelope {
        request_id: 44,
        payload: SearchPlaneQueryIpcRequest::Semantic(SemanticQueryRequest {
            query_text: "typed semantic parser".to_string(),
            constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
            generation: Some(GenerationPin::new(repo(), revision(), generation())),
            generation_selector: None,
            lexical_scope: None,
            top_k: 1,
        }),
    };

    if !wait_until(READINESS_TIMEOUT, || {
        send_query_request(&socket, &req)
            .map(|resp| !matches!(resp.payload, SearchPlaneQueryIpcResponse::Error(_)))
            .unwrap_or(false)
    }) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("semantic explicit hash query never became ready".into());
    }

    let response = send_query_request(&socket, &req)?;
    let semantic = match response.payload {
        SearchPlaneQueryIpcResponse::Semantic(semantic) => semantic,
        other => {
            shutdown.store(true, Ordering::Release);
            drop(join.join());
            return Err(format!("expected semantic response, got {other:?}").into());
        }
    };
    if semantic.generation != GenerationPin::new(repo(), revision(), generation()) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("semantic explicit hash response did not echo request pin".into());
    }
    if !matches!(semantic.results.as_slice(), [only] if only.candidate_id == "alpha") {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!(
            "expected explicit-hash semantic query to rank alpha first, got {semantic:?}"
        )
        .into());
    }

    stop_runtime(shutdown, join)
}

#[test]
fn semantic_query_rejects_generation_pin_mismatch_with_lexical_scope() -> TestResult {
    let dir = tempfile::tempdir()?;
    let state_root = dir.path();
    let config = build_config(state_root);
    let runtime = build_runtime(config)?;
    let socket = runtime.query_server.socket_path().to_path_buf();
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_for_drive = Arc::clone(&shutdown);
    let join = thread::Builder::new()
        .name("searchd-semantic-pin-mismatch-test".into())
        .spawn(move || drive(runtime, &shutdown_for_drive))?;

    if !wait_until(Duration::from_secs(2), || socket.exists()) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("socket never appeared".into());
    }

    let response = send_query_request(
        &socket,
        &SearchPlaneQueryIpcRequestEnvelope {
            request_id: 43,
            payload: SearchPlaneQueryIpcRequest::Semantic(SemanticQueryRequest {
                query_text: "scope".to_string(),
                constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                generation: Some(GenerationPin::new(repo(), revision(), generation())),
                generation_selector: None,
                lexical_scope: Some(TextQueryRequest {
                    syntax: TextQuerySyntax::Sourcegraph,
                    query_text: "scope".to_string(),
                    constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                    generation: Some(GenerationPin::new(
                        repo(),
                        revision(),
                        ManifestGeneration::new(8),
                    )),
                    generation_selector: None,
                    top_k: 1,
                }),
                top_k: 1,
            }),
        },
    )?;
    let err = match response.payload {
        SearchPlaneQueryIpcResponse::Error(err) => err,
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
    if !err
        .message
        .contains("semantic: scope generation does not match semantic request generation")
    {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!("unexpected mismatch message: {}", err.message).into());
    }

    shutdown.store(true, Ordering::Release);
    match join.join() {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(e.into()),
        Err(panic) => Err(format!("driver panic: {panic:?}").into()),
    }
}

#[test]
fn semantic_query_executes_scoped_unindexed_lexical_scope() -> TestResult {
    let dir = tempfile::tempdir()?;
    let state_root = dir.path();
    let (socket, ingest_socket, shutdown, join) =
        start_runtime(state_root, "searchd-semantic-unindexed-scope-test")?;
    let alpha = chunk_record("alpha", "semantic alpha")?;
    publish_search_corpus_chunks(&ingest_socket, vec![alpha], None)?;
    seal_lexical(&ingest_socket)?;

    let pin = GenerationPin::new(repo(), revision(), generation());
    let req = SearchPlaneQueryIpcRequestEnvelope {
        request_id: 44,
        payload: SearchPlaneQueryIpcRequest::Semantic(SemanticQueryRequest {
            query_text: "alpha".to_string(),
            constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
            generation: Some(pin.clone()),
            generation_selector: None,
            lexical_scope: Some(TextQueryRequest {
                syntax: TextQuerySyntax::Sourcegraph,
                query_text: "index:no semantic".to_string(),
                constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                generation: Some(pin.clone()),
                generation_selector: None,
                top_k: 1,
            }),
            top_k: 1,
        }),
    };

    if !wait_until(READINESS_TIMEOUT, || {
        send_query_request(&socket, &req)
            .map(|resp| !matches!(resp.payload, SearchPlaneQueryIpcResponse::Error(_)))
            .unwrap_or(false)
    }) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("semantic unindexed scope query never became ready".into());
    }

    let response = send_query_request(&socket, &req)?;
    let results = match response.payload {
        SearchPlaneQueryIpcResponse::Semantic(semantic) => {
            if semantic.generation != pin {
                shutdown.store(true, Ordering::Release);
                drop(join.join());
                return Err("semantic response generation did not echo request pin".into());
            }
            semantic.results
        }
        other => {
            shutdown.store(true, Ordering::Release);
            drop(join.join());
            return Err(format!("expected Semantic, got {other:?}").into());
        }
    };
    let ids: Vec<String> = results
        .iter()
        .map(|candidate| candidate.candidate_id.clone())
        .collect();
    if ids != ["alpha".to_string()] {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!("expected scoped semantic intersection [alpha], got {ids:?}").into());
    }

    stop_runtime(shutdown, join)
}

#[test]
fn semantic_query_with_lexical_scope_returns_intersection_only() -> TestResult {
    let dir = tempfile::tempdir()?;
    let state_root = dir.path();
    let pin = GenerationPin::new(repo(), revision(), generation());
    let (socket, ingest_socket, shutdown, join) =
        start_runtime(state_root, "searchd-semantic-scope-test")?;
    let alpha = chunk_record("alpha", "scope needle")?;
    let beta = chunk_record("beta", "scope miss")?;
    let gamma = chunk_record("gamma", "outside needle")?;
    publish_search_corpus_chunks(&ingest_socket, vec![alpha, beta, gamma], None)?;
    seal_lexical(&ingest_socket)?;

    let req = SearchPlaneQueryIpcRequestEnvelope {
        request_id: 41,
        payload: SearchPlaneQueryIpcRequest::Semantic(SemanticQueryRequest {
            query_text: "needle".to_string(),
            constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
            generation: Some(pin.clone()),
            generation_selector: None,
            lexical_scope: Some(TextQueryRequest {
                syntax: TextQuerySyntax::Sourcegraph,
                query_text: "scope needle".to_string(),
                constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                generation: Some(pin),
                generation_selector: None,
                top_k: 2,
            }),
            top_k: 2,
        }),
    };

    if !wait_until(READINESS_TIMEOUT, || {
        send_query_request(&socket, &req)
            .map(|resp| !matches!(resp.payload, SearchPlaneQueryIpcResponse::Error(_)))
            .unwrap_or(false)
    }) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("semantic scoped query never became ready".into());
    }

    let response = send_query_request(&socket, &req)?;
    let results = match response.payload {
        SearchPlaneQueryIpcResponse::Semantic(semantic) => semantic.results,
        other => {
            shutdown.store(true, Ordering::Release);
            drop(join.join());
            return Err(format!("expected Semantic, got {other:?}").into());
        }
    };
    let ids: Vec<String> = results
        .iter()
        .map(|candidate| candidate.candidate_id.clone())
        .collect();
    if ids != ["alpha".to_string()] {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!("expected scoped semantic intersection [alpha], got {ids:?}").into());
    }

    stop_runtime(shutdown, join)
}

#[test]
fn semantic_scoped_query_ignores_out_of_scope_global_nearest_hit() -> TestResult {
    let dir = tempfile::tempdir()?;
    let state_root = dir.path();
    let pin = GenerationPin::new(repo(), revision(), generation());
    let (socket, ingest_socket, shutdown, join) =
        start_runtime(state_root, "searchd-semantic-scope-starvation-test")?;
    let alpha = chunk_record("alpha", "focus alpha")?;
    let beta = chunk_record("beta", "scope focus")?;
    let gamma = chunk_record("gamma", "scope gamma")?;
    publish_search_corpus_chunks(&ingest_socket, vec![alpha, beta, gamma], None)?;
    seal_lexical(&ingest_socket)?;

    let req = SearchPlaneQueryIpcRequestEnvelope {
        request_id: 42,
        payload: SearchPlaneQueryIpcRequest::Semantic(SemanticQueryRequest {
            query_text: "focus alpha".to_string(),
            constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
            generation: Some(pin.clone()),
            generation_selector: None,
            lexical_scope: Some(TextQueryRequest {
                syntax: TextQuerySyntax::Sourcegraph,
                query_text: "scope".to_string(),
                constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                generation: Some(pin),
                generation_selector: None,
                top_k: 1,
            }),
            top_k: 1,
        }),
    };

    if !wait_until(READINESS_TIMEOUT, || {
        send_query_request(&socket, &req)
            .map(|resp| !matches!(resp.payload, SearchPlaneQueryIpcResponse::Error(_)))
            .unwrap_or(false)
    }) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("semantic scoped query never became ready".into());
    }

    let response = send_query_request(&socket, &req)?;
    let results = match response.payload {
        SearchPlaneQueryIpcResponse::Semantic(semantic) => semantic.results,
        other => {
            shutdown.store(true, Ordering::Release);
            drop(join.join());
            return Err(format!("expected Semantic, got {other:?}").into());
        }
    };
    let ids: Vec<String> = results
        .iter()
        .map(|candidate| candidate.candidate_id.clone())
        .collect();
    if ids != ["beta".to_string()] {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!("expected scoped semantic [beta], got {ids:?}").into());
    }

    stop_runtime(shutdown, join)
}

#[test]
fn semantic_query_rejects_empty_text_with_typed_code() -> TestResult {
    let dir = tempfile::tempdir()?;
    let state_root = dir.path();
    let (socket, ingest_socket, shutdown, join) =
        start_runtime(state_root, "searchd-sem-empty-query-test")?;
    publish_search_corpus_chunks(
        &ingest_socket,
        vec![chunk_record("alpha", "semantic alpha")?],
        None,
    )?;
    seal_lexical(&ingest_socket)?;

    let req = SearchPlaneQueryIpcRequestEnvelope {
        request_id: 43,
        payload: SearchPlaneQueryIpcRequest::Semantic(SemanticQueryRequest {
            query_text: "!!!".to_string(),
            constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
            generation: Some(GenerationPin::new(repo(), revision(), generation())),
            generation_selector: None,
            lexical_scope: None,
            top_k: 3,
        }),
    };

    if !wait_until(READINESS_TIMEOUT, || {
        send_query_request(&socket, &req)
            .map(|resp| match resp.payload {
                SearchPlaneQueryIpcResponse::Error(err) => err.code != "NOT_READY",
                _ => true,
            })
            .unwrap_or(false)
    }) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("semantic empty-query request never progressed past NOT_READY".into());
    }

    let response = send_query_request(&socket, &req)?;
    let err = match response.payload {
        SearchPlaneQueryIpcResponse::Error(err) => err,
        other => {
            shutdown.store(true, Ordering::Release);
            drop(join.join());
            return Err(format!("expected Error, got {other:?}").into());
        }
    };
    if err.code != "EMPTY_QUERY" {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!("expected EMPTY_QUERY, got {}", err.code).into());
    }

    stop_runtime(shutdown, join)
}

#[test]
fn semantic_query_fails_closed_when_runtime_has_no_query_embedder() -> TestResult {
    let dir = tempfile::tempdir()?;
    let state_root = dir.path();
    let mut config = SearchdConfig::from_state_root(state_root.to_path_buf())
        .try_with_search_corpus_history_retention_limits(
            8,
            16 * 1024 * 1024,
            128,
            256 * 1024 * 1024,
        )
        .expect("valid test retention policy")
        .with_provider_unavailable_query_text_embedder();
    let (query_socket, control_socket) = unique_socket_paths();
    config = SearchdConfig::with_socket_overrides(config, query_socket, control_socket);
    let runtime = build_runtime(config)?;
    let socket = runtime.query_server.socket_path().to_path_buf();
    let ingest_socket = runtime.ingest_server.socket_path().to_path_buf();
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_for_drive = Arc::clone(&shutdown);
    let join = thread::Builder::new()
        .name("searchd-sem-provider-unavailable-test".into())
        .spawn(move || drive(runtime, &shutdown_for_drive))?;

    if !wait_until(SOCKET_APPEAR_TIMEOUT, || {
        socket.exists() && ingest_socket.exists()
    }) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("semantic provider-unavailable sockets never appeared".into());
    }

    let alpha = chunk_record("alpha", "semantic alpha")?;
    publish_search_corpus_chunks(&ingest_socket, vec![alpha], None)?;
    seal_lexical(&ingest_socket)?;

    let req = SearchPlaneQueryIpcRequestEnvelope {
        request_id: 44,
        payload: SearchPlaneQueryIpcRequest::Semantic(SemanticQueryRequest {
            query_text: "semantic meaning".to_string(),
            constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
            generation: Some(GenerationPin::new(repo(), revision(), generation())),
            generation_selector: None,
            lexical_scope: None,
            top_k: 3,
        }),
    };

    if !wait_until(READINESS_TIMEOUT, || {
        send_query_request(&socket, &req)
            .map(|resp| match resp.payload {
                SearchPlaneQueryIpcResponse::Error(err) => err.code != "NOT_READY",
                _ => true,
            })
            .unwrap_or(false)
    }) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("semantic provider-unavailable query never progressed past NOT_READY".into());
    }

    let response = send_query_request(&socket, &req)?;
    let err = match response.payload {
        SearchPlaneQueryIpcResponse::Error(err) => err,
        other => {
            shutdown.store(true, Ordering::Release);
            drop(join.join());
            return Err(format!("expected Error, got {other:?}").into());
        }
    };
    if err.code != "SEM_PROVIDER_UNAVAILABLE" {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!("expected SEM_PROVIDER_UNAVAILABLE, got {}", err.code).into());
    }

    stop_runtime(shutdown, join)
}

#[test]
fn hybrid_query_rejects_zero_top_k_with_typed_code() -> TestResult {
    let dir = tempfile::tempdir()?;
    let state_root = dir.path();
    let config = build_config(state_root);
    let runtime = build_runtime(config)?;
    let socket = runtime.query_server.socket_path().to_path_buf();
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_for_drive = Arc::clone(&shutdown);
    let join = thread::Builder::new()
        .name("searchd-hybrid-top-k-test".into())
        .spawn(move || drive(runtime, &shutdown_for_drive))?;

    if !wait_until(Duration::from_secs(2), || socket.exists()) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("socket never appeared".into());
    }

    let response = send_query_request(
        &socket,
        &SearchPlaneQueryIpcRequestEnvelope {
            request_id: 44,
            payload: SearchPlaneQueryIpcRequest::Hybrid(HybridQueryRequest {
                text_query: TextQueryRequest {
                    syntax: TextQuerySyntax::Sourcegraph,
                    query_text: "needle".to_string(),
                    constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                    generation: Some(GenerationPin::new(repo(), revision(), generation())),
                    generation_selector: None,
                    top_k: 50,
                },
                semantic_query_text: "needle".to_string(),
                generation: Some(GenerationPin::new(repo(), revision(), generation())),
                generation_selector: None,
                top_k: 0,
            }),
        },
    )?;
    let err = match response.payload {
        SearchPlaneQueryIpcResponse::Error(err) => err,
        other => {
            shutdown.store(true, Ordering::Release);
            drop(join.join());
            return Err(format!("expected Error, got {other:?}").into());
        }
    };
    // QI-BB-025: `top_k` is one route-independent contract; the hybrid route
    // no longer has a private code for it.
    if err.code != quanta_index_contract::TOP_K_OUT_OF_RANGE_CODE {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!(
            "expected {}, got {}",
            quanta_index_contract::TOP_K_OUT_OF_RANGE_CODE,
            err.code
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

/// QI-BB-018: hybrid is two independent lanes fused, not a dense re-rank
/// of lexical recall.
///
/// `alpha` matches nothing the lexical lane looks for (`riddle`) but is
/// what the dense lane looks for (`focus alpha`); it must reach the top-k
/// on dense relevance alone, while `beta` — found by both lanes — stays
/// first.
#[test]
fn hybrid_query_admits_a_semantic_only_relevant_hit_beside_the_lexical_hits() -> TestResult {
    let dir = tempfile::tempdir()?;
    let state_root = dir.path();
    let (socket, ingest_socket, shutdown, join) =
        start_runtime(state_root, "searchd-hybrid-outsider-test")?;
    // Lexical lane (`riddle`): beta only. Dense lane (`focus alpha`): alpha
    // first, beta second, gamma last. Fused at top_k=2: beta (both lanes),
    // then alpha on dense relevance alone; gamma, ranked last by the one
    // lane that saw it, stays out.
    let alpha = chunk_record("alpha", "focus alpha")?;
    let beta = chunk_record("beta", "riddle focus")?;
    let gamma = chunk_record("gamma", "scope gamma")?;
    publish_search_corpus_chunks(&ingest_socket, vec![alpha, beta, gamma], None)?;
    seal_lexical(&ingest_socket)?;

    let pin = GenerationPin::new(repo(), revision(), generation());
    let req = SearchPlaneQueryIpcRequestEnvelope {
        request_id: 45,
        payload: SearchPlaneQueryIpcRequest::Hybrid(HybridQueryRequest {
            text_query: TextQueryRequest {
                syntax: TextQuerySyntax::Sourcegraph,
                query_text: "riddle".to_string(),
                constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                generation: Some(pin.clone()),
                generation_selector: None,
                top_k: 50,
            },
            semantic_query_text: "focus alpha".to_string(),
            generation: Some(pin),
            generation_selector: None,
            top_k: 2,
        }),
    };

    if !wait_until(READINESS_TIMEOUT, || {
        send_query_request(&socket, &req)
            .map(|resp| !matches!(resp.payload, SearchPlaneQueryIpcResponse::Error(_)))
            .unwrap_or(false)
    }) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("hybrid query never became ready".into());
    }

    let response = send_query_request(&socket, &req)?;
    let results = match response.payload {
        SearchPlaneQueryIpcResponse::Hybrid(hybrid) => hybrid.results,
        other => {
            shutdown.store(true, Ordering::Release);
            drop(join.join());
            return Err(format!("expected Hybrid, got {other:?}").into());
        }
    };
    let ids: Vec<String> = results
        .iter()
        .map(|candidate| candidate.candidate_id.clone())
        .collect();
    if ids.first().map(String::as_str) != Some("beta") {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!("expected beta top, got {ids:?}").into());
    }
    if !ids.iter().any(|id| id == "alpha") {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!(
            "the dense-only relevant hit must enter the hybrid top-k, got {ids:?}"
        )
        .into());
    }
    if ids.iter().any(|id| id == "gamma") {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!(
            "a hit one lane ranked last must not outrank the fused pair at top_k=2: {ids:?}"
        )
        .into());
    }

    stop_runtime(shutdown, join)
}

#[test]
fn hybrid_query_repeated_tied_scope_query_keeps_stable_order() -> TestResult {
    let dir = tempfile::tempdir()?;
    let state_root = dir.path();
    let (socket, ingest_socket, shutdown, join) =
        start_runtime(state_root, "searchd-hybrid-tie-determinism-test")?;
    let alpha = chunk_record("alpha", "scope tie")?;
    let beta = chunk_record("beta", "scope tie")?;
    publish_search_corpus_chunks(&ingest_socket, vec![alpha, beta], None)?;
    seal_lexical(&ingest_socket)?;

    let pin = GenerationPin::new(repo(), revision(), generation());
    let request = SearchPlaneQueryIpcRequestEnvelope {
        request_id: 46,
        payload: SearchPlaneQueryIpcRequest::Hybrid(HybridQueryRequest {
            text_query: TextQueryRequest {
                syntax: TextQuerySyntax::Native,
                query_text: "scope tie".to_string(),
                constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                generation: Some(pin.clone()),
                generation_selector: None,
                top_k: 50,
            },
            semantic_query_text: "scope tie".to_string(),
            generation: Some(pin),
            generation_selector: None,
            top_k: 2,
        }),
    };

    if !wait_until(READINESS_TIMEOUT, || {
        send_query_request(&socket, &request)
            .map(|resp| !matches!(resp.payload, SearchPlaneQueryIpcResponse::Error(_)))
            .unwrap_or(false)
    }) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("hybrid tie determinism query never became ready".into());
    }

    let query_ids =
        |response: SearchPlaneQueryIpcResponseEnvelope| -> Result<Vec<String>, Box<dyn Error>> {
            match response.payload {
                SearchPlaneQueryIpcResponse::Hybrid(hybrid) => Ok(hybrid
                    .results
                    .into_iter()
                    .map(|candidate| candidate.candidate_id)
                    .collect()),
                other => Err(format!("expected Hybrid, got {other:?}").into()),
            }
        };

    let first_ids = query_ids(send_query_request(&socket, &request)?)?;
    let second_ids = query_ids(send_query_request(&socket, &request)?)?;
    if first_ids != second_ids {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!(
            "hybrid tie ordering drifted across repeated queries: first={first_ids:?} second={second_ids:?}"
        )
        .into());
    }
    if first_ids.len() != 2 {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!("expected 2 tied hybrid results, got {first_ids:?}").into());
    }

    stop_runtime(shutdown, join)
}

#[test]
fn structural_query_returns_typed_generation_not_ready_error() -> TestResult {
    let dir = tempfile::tempdir()?;
    let state_root = dir.path();
    let config = build_config(state_root);
    let runtime = build_runtime(config)?;
    let socket = runtime.query_server.socket_path().to_path_buf();
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_for_drive = Arc::clone(&shutdown);
    let join = thread::Builder::new()
        .name("searchd-structural-test".into())
        .spawn(move || drive(runtime, &shutdown_for_drive))?;

    if !wait_until(Duration::from_secs(2), || socket.exists()) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("socket never appeared".into());
    }

    let response = send_query_request(
        &socket,
        &SearchPlaneQueryIpcRequestEnvelope {
            request_id: 42,
            payload: SearchPlaneQueryIpcRequest::Structural(StructuralQueryRequest {
                text_query: TextQueryRequest {
                    syntax: TextQuerySyntax::Native,
                    query_text: "match { :[x] }".to_string(),
                    constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                    generation: Some(GenerationPin::new(repo(), revision(), generation())),
                    generation_selector: None,
                    top_k: 50,
                },
            }),
        },
    )?;
    let err = match response.payload {
        SearchPlaneQueryIpcResponse::Error(err) => err,
        other => {
            shutdown.store(true, Ordering::Release);
            drop(join.join());
            return Err(format!("expected Error, got {other:?}").into());
        }
    };
    if err.code != "STR_GENERATION_NOT_READY" {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!("expected STR_GENERATION_NOT_READY, got {}", err.code).into());
    }
    if !err.message.contains("not yet materialized") {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!(
            "expected generation-not-ready structural message, got {}",
            err.message
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
fn structural_query_returns_typed_shard_unavailable_error() -> TestResult {
    let dir = tempfile::tempdir()?;
    let state_root = dir.path();
    let (socket, ingest_socket, shutdown, join) =
        start_runtime(state_root, "searchd-structural-shard-unavailable-test")?;
    publish_search_corpus_chunks(
        &ingest_socket,
        vec![
            chunk_record_with_metadata("chunk-tree", "src/lib.rs", "rust", 1, 1, "fn main() {}")?,
            // Keep one search-owned semantic source alive after orphaning the
            // structural chunk below. The generation contract requires the
            // RawCodeFallback corpus to remain present at seal time; this
            // sentinel is outside the structural scope under test.
            chunk_record_with_metadata(
                "chunk-semantic-sentinel",
                "src/semantic_sentinel.rs",
                "rust",
                1,
                1,
                "fn semantic_sentinel() {}",
            )?,
        ],
        Some(b"manifest".to_vec()),
    )?;
    publish_structural_scope(
        &ingest_socket,
        "src/lib.rs",
        vec![StructuralTreeRecord {
            chunk_id: ChunkId::new("chunk-tree"),
            record: ParseTreeRecord {
                wire_version: 1,
                lang: LanguageCode::new("rust").map_err(|err| -> Box<dyn Error> {
                    format!("invalid tree lang: {err}").into()
                })?,
                root: ParseNode {
                    kind: "function_item".to_string().into_boxed_str(),
                    byte_start: 0,
                    byte_end: 10,
                    children: Vec::new(),
                },
                source_hash: compute_parse_tree_source_hash("fn main() {}"),
                role_tag_schema_version: 1,
                role_tags: vec![ParseRoleTag {
                    role: "item".to_string().into_boxed_str(),
                    byte_start: 0,
                    byte_end: 10,
                }],
            },
        }],
    )?;
    tombstone_lexical_scopes(&ingest_socket, &["src/lib.rs"])?;
    seal_lexical(&ingest_socket)?;
    seal_structural(&ingest_socket)?;

    let request = SearchPlaneQueryIpcRequestEnvelope {
        request_id: 44,
        payload: SearchPlaneQueryIpcRequest::Structural(StructuralQueryRequest {
            text_query: TextQueryRequest {
                syntax: TextQuerySyntax::Native,
                query_text: "match { :[x] }".to_string(),
                constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                generation: Some(GenerationPin::new(repo(), revision(), generation())),
                generation_selector: None,
                top_k: 50,
            },
        }),
    };
    let mut observed: Option<String> = None;
    let saw_expected = wait_until(READINESS_TIMEOUT, || {
        match send_query_request(&socket, &request) {
            Ok(response) => match response.payload {
                SearchPlaneQueryIpcResponse::Error(err) => {
                    observed = Some(err.code.clone());
                    err.code == "STR_SHARD_UNAVAILABLE"
                }
                other => {
                    observed = Some(format!("{other:?}"));
                    false
                }
            },
            Err(err) => {
                observed = Some(format!("{err}"));
                false
            }
        }
    });
    if !saw_expected {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!(
            "expected STR_SHARD_UNAVAILABLE after orphaning structural chunk authority, observed {observed:?}"
        )
        .into());
    }

    stop_runtime(shutdown, join)
}

/// Composition-wiring assertion (MINOR 6).
///
/// The structural producer wired in `searchd::app::runtime` must route
/// through the domain port and emit a typed error response — never a
/// panic, never a transport error, never a candidate list. The exact
/// error code is *loosely* asserted here so this gate survives Track 2's
/// current live producer adapter should return a typed readiness error when
/// no structural generation has been materialized yet.
/// The strict-code assertion lives in
/// `structural_query_returns_typed_generation_not_ready_error` above and
/// this looser wiring check ensures the composition root still emits a
/// typed structural code rather than panicking or returning a payload.
#[test]
fn structural_query_composition_wiring_emits_typed_error() -> TestResult {
    let dir = tempfile::tempdir()?;
    let state_root = dir.path();
    let config = build_config(state_root);
    let runtime = build_runtime(config)?;
    let socket = runtime.query_server.socket_path().to_path_buf();
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_for_drive = Arc::clone(&shutdown);
    let join = thread::Builder::new()
        .name("searchd-structural-wiring-test".into())
        .spawn(move || drive(runtime, &shutdown_for_drive))?;

    if !wait_until(Duration::from_secs(2), || socket.exists()) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("socket never appeared".into());
    }

    let response = send_query_request(
        &socket,
        &SearchPlaneQueryIpcRequestEnvelope {
            request_id: 43,
            payload: SearchPlaneQueryIpcRequest::Structural(StructuralQueryRequest {
                text_query: TextQueryRequest {
                    syntax: TextQuerySyntax::Native,
                    query_text: "match { :[x] }".to_string(),
                    constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                    generation: Some(GenerationPin::new(repo(), revision(), generation())),
                    generation_selector: None,
                    top_k: 50,
                },
            }),
        },
    )?;

    let result: Result<(), String> = match response.payload {
        SearchPlaneQueryIpcResponse::Error(err) => {
            // Loose typed-code surface assertion: the dispatcher must
            // surface SOME structural-shaped typed error. Acceptable
            // shape today: STR_GENERATION_NOT_READY while no structural
            // materialization exists. Anything else is a genuine wiring
            // regression.
            let code = err.code.as_str();
            if code == "STR_GENERATION_NOT_READY" {
                Ok(())
            } else {
                Err(format!(
                    "structural composition wiring: expected typed structural error \
                     (STR_GENERATION_NOT_READY), got code={code} \
                     message={}",
                    err.message
                ))
            }
        }
        SearchPlaneQueryIpcResponse::Structural(_) => Err(
            "structural composition wiring: expected Error, got Structural \
                 (no structural generation was materialized for this test)"
                .to_string(),
        ),
        other => Err(format!(
            "structural composition wiring: expected Error response, got {other:?}"
        )),
    };

    shutdown.store(true, Ordering::Release);
    drop(join.join());
    result.map_err(Into::into)
}

#[test]
fn structural_sourcegraph_query_returns_match_after_parse_tree_ingest() -> TestResult {
    let dir = tempfile::tempdir()?;
    let state_root = dir.path();
    let (socket, ingest_socket, shutdown, join) =
        start_runtime(state_root, "searchd-structural-sourcegraph-success-test")?;
    publish_structural_ready_fixture(&ingest_socket)?;
    seal_lexical(&ingest_socket)?;
    seal_structural(&ingest_socket)?;

    let pin = GenerationPin::new(repo(), revision(), generation());
    let request = SearchPlaneQueryIpcRequestEnvelope {
        request_id: 46,
        payload: SearchPlaneQueryIpcRequest::Structural(StructuralQueryRequest {
            text_query: TextQueryRequest {
                syntax: TextQuerySyntax::Sourcegraph,
                query_text: r#"repo:repo-int path:src/lib.rs lang:rust patterntype:structural "function_item { { identifier :[name] } }""#.to_string(),
                constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                generation: Some(pin.clone()),
                generation_selector: None,
                top_k: 50,
            },
        }),
    };
    let mut observed: Option<String> = None;
    let saw_ready = wait_until(READINESS_TIMEOUT, || {
        match send_query_request(&socket, &request) {
            Ok(response) => match response.payload {
                SearchPlaneQueryIpcResponse::Structural(structural) => {
                    observed = Some(format!("{structural:?}"));
                    structural.generation == pin && structural.results.len() == 1
                }
                SearchPlaneQueryIpcResponse::Error(err)
                    if err.code == "NOT_READY" || err.code == "STR_GENERATION_NOT_READY" =>
                {
                    observed = Some(err.code);
                    false
                }
                other => {
                    observed = Some(format!("{other:?}"));
                    false
                }
            },
            Err(err) => {
                observed = Some(err.to_string());
                false
            }
        }
    });
    if !saw_ready {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!(
            "structural Sourcegraph query never became ready; observed {observed:?}"
        )
        .into());
    }

    let response = send_query_request(&socket, &request)?;
    let structural = match response.payload {
        SearchPlaneQueryIpcResponse::Structural(structural) => structural,
        other => {
            shutdown.store(true, Ordering::Release);
            drop(join.join());
            return Err(format!("expected Structural, got {other:?}").into());
        }
    };
    if structural.generation != pin || structural.results.len() != 1 {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!("unexpected structural Sourcegraph response: {structural:?}").into());
    }
    let candidate = structural
        .results
        .first()
        .ok_or_else(|| "missing structural Sourcegraph candidate".to_string())?;
    let binding = candidate
        .bindings
        .first()
        .ok_or_else(|| "missing structural Sourcegraph binding".to_string())?;
    if candidate.candidate_id != "chunk-tree"
        || binding.metavariable != "name"
        || binding.start_byte != 3
        || binding.end_byte != 7
    {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(
            format!("unexpected structural Sourcegraph candidate/binding: {candidate:?}").into(),
        );
    }

    stop_runtime(shutdown, join)
}

#[test]
fn structural_sourcegraph_regex_query_returns_match_after_parse_tree_ingest() -> TestResult {
    let dir = tempfile::tempdir()?;
    let state_root = dir.path();
    let (socket, ingest_socket, shutdown, join) = start_runtime(
        state_root,
        "searchd-structural-sourcegraph-regex-success-test",
    )?;
    publish_structural_ready_fixture(&ingest_socket)?;
    seal_lexical(&ingest_socket)?;
    seal_structural(&ingest_socket)?;

    let pin = GenerationPin::new(repo(), revision(), generation());
    let request = SearchPlaneQueryIpcRequestEnvelope {
        request_id: 46,
        payload: SearchPlaneQueryIpcRequest::Structural(StructuralQueryRequest {
            text_query: TextQueryRequest {
                syntax: TextQuerySyntax::Sourcegraph,
                query_text:
                    r"repo:repo-int path:src/lib.rs lang:rust patterntype:structural /^main$/"
                        .to_string(),
                constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                generation: Some(pin.clone()),
                generation_selector: None,
                top_k: 50,
            },
        }),
    };
    let mut observed: Option<String> = None;
    let saw_ready = wait_until(READINESS_TIMEOUT, || {
        match send_query_request(&socket, &request) {
            Ok(response) => match response.payload {
                SearchPlaneQueryIpcResponse::Structural(structural) => {
                    observed = Some(format!("{structural:?}"));
                    structural.generation == pin && structural.results.len() == 1
                }
                SearchPlaneQueryIpcResponse::Error(err)
                    if err.code == "NOT_READY" || err.code == "STR_GENERATION_NOT_READY" =>
                {
                    observed = Some(err.code);
                    false
                }
                other => {
                    observed = Some(format!("{other:?}"));
                    false
                }
            },
            Err(err) => {
                observed = Some(err.to_string());
                false
            }
        }
    });
    if !saw_ready {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!(
            "structural Sourcegraph regex query never became ready; observed {observed:?}"
        )
        .into());
    }

    let response = send_query_request(&socket, &request)?;
    let structural = match response.payload {
        SearchPlaneQueryIpcResponse::Structural(structural) => structural,
        other => {
            shutdown.store(true, Ordering::Release);
            drop(join.join());
            return Err(format!("expected Structural, got {other:?}").into());
        }
    };
    let candidate = structural
        .results
        .first()
        .ok_or_else(|| "missing structural Sourcegraph regex candidate".to_string())?;
    let binding = candidate
        .bindings
        .first()
        .ok_or_else(|| "missing structural Sourcegraph regex binding".to_string())?;
    if candidate.candidate_id != "chunk-tree"
        || !binding.metavariable.starts_with("__sg_regex_")
        || binding.start_byte != 3
        || binding.end_byte != 7
    {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!(
            "unexpected structural Sourcegraph regex candidate/binding: {candidate:?}"
        )
        .into());
    }

    stop_runtime(shutdown, join)
}

#[test]
fn structural_sourcegraph_query_requires_structural_pattern_type() -> TestResult {
    let dir = tempfile::tempdir()?;
    let state_root = dir.path();
    let config = build_config(state_root);
    let runtime = build_runtime(config)?;
    let socket = runtime.query_server.socket_path().to_path_buf();
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_for_drive = Arc::clone(&shutdown);
    let join = thread::Builder::new()
        .name("searchd-structural-sourcegraph-pattern-type-test".into())
        .spawn(move || drive(runtime, &shutdown_for_drive))?;

    if !wait_until(Duration::from_secs(2), || socket.exists()) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("socket never appeared".into());
    }

    let response = send_query_request(
        &socket,
        &SearchPlaneQueryIpcRequestEnvelope {
            request_id: 47,
            payload: SearchPlaneQueryIpcRequest::Structural(StructuralQueryRequest {
                text_query: TextQueryRequest {
                    syntax: TextQuerySyntax::Sourcegraph,
                    query_text: r#""function_item""#.to_string(),
                    constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                    generation: Some(GenerationPin::new(repo(), revision(), generation())),
                    generation_selector: None,
                    top_k: 50,
                },
            }),
        },
    )?;
    let err = match response.payload {
        SearchPlaneQueryIpcResponse::Error(err) => err,
        other => {
            shutdown.store(true, Ordering::Release);
            drop(join.join());
            return Err(format!("expected Error, got {other:?}").into());
        }
    };
    if err.code != "BRIDGE_TRANSLATE_FAIL" || !err.message.contains("patterntype:structural") {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!(
            "expected BRIDGE_TRANSLATE_FAIL structural pattern-type error, got {err:?}"
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
fn structural_sourcegraph_query_rejects_select_filter() -> TestResult {
    let dir = tempfile::tempdir()?;
    let state_root = dir.path();
    let (socket, ingest_socket, shutdown, join) = start_runtime(
        state_root,
        "searchd-structural-sourcegraph-select-filter-test",
    )?;
    publish_structural_ready_fixture(&ingest_socket)?;
    seal_lexical(&ingest_socket)?;
    seal_structural(&ingest_socket)?;

    let request = SearchPlaneQueryIpcRequestEnvelope {
        request_id: 48,
        payload: SearchPlaneQueryIpcRequest::Structural(StructuralQueryRequest {
            text_query: TextQueryRequest {
                syntax: TextQuerySyntax::Sourcegraph,
                query_text: r#"select:repo patterntype:structural "function_item""#.to_string(),
                constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                generation: Some(GenerationPin::new(repo(), revision(), generation())),
                generation_selector: None,
                top_k: 50,
            },
        }),
    };
    let mut observed: Option<String> = None;
    let saw_expected = wait_until(READINESS_TIMEOUT, || {
        match send_query_request(&socket, &request) {
            Ok(response) => match response.payload {
                SearchPlaneQueryIpcResponse::Error(err)
                    if err.code == "NOT_READY" || err.code == "STR_GENERATION_NOT_READY" =>
                {
                    observed = Some(err.code);
                    false
                }
                SearchPlaneQueryIpcResponse::Error(err) => {
                    observed = Some(format!("{err:?}"));
                    err.code == "STR_INVALID_REQUEST" && err.message.contains("filter `select`")
                }
                other => {
                    observed = Some(format!("{other:?}"));
                    false
                }
            },
            Err(err) => {
                observed = Some(err.to_string());
                false
            }
        }
    });
    if !saw_expected {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!(
            "expected STR_INVALID_REQUEST for structural SG select filter, observed {observed:?}"
        )
        .into());
    }

    stop_runtime(shutdown, join)
}

#[test]
fn structural_sourcegraph_query_rejects_timeout_filter() -> TestResult {
    let dir = tempfile::tempdir()?;
    let state_root = dir.path();
    let config = build_config(state_root);
    let runtime = build_runtime(config)?;
    let socket = runtime.query_server.socket_path().to_path_buf();
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_for_drive = Arc::clone(&shutdown);
    let join = thread::Builder::new()
        .name("searchd-structural-sourcegraph-timeout-filter-test".into())
        .spawn(move || drive(runtime, &shutdown_for_drive))?;

    if !wait_until(Duration::from_secs(2), || socket.exists()) {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err("socket never appeared".into());
    }

    let response = send_query_request(
        &socket,
        &SearchPlaneQueryIpcRequestEnvelope {
            request_id: 49,
            payload: SearchPlaneQueryIpcRequest::Structural(StructuralQueryRequest {
                text_query: TextQueryRequest {
                    syntax: TextQuerySyntax::Sourcegraph,
                    query_text: r#"timeout:0ms patterntype:structural "function_item""#.to_string(),
                    constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                    generation: Some(GenerationPin::new(repo(), revision(), generation())),
                    generation_selector: None,
                    top_k: 50,
                },
            }),
        },
    )?;
    let err = match response.payload {
        SearchPlaneQueryIpcResponse::Error(err) => err,
        other => {
            shutdown.store(true, Ordering::Release);
            drop(join.join());
            return Err(format!("expected Error, got {other:?}").into());
        }
    };
    if err.code != "STR_INVALID_REQUEST" || !err.message.contains("timeout option") {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(
            format!("expected STR_INVALID_REQUEST structural timeout error, got {err:?}").into(),
        );
    }

    shutdown.store(true, Ordering::Release);
    match join.join() {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(e.into()),
        Err(panic) => Err(format!("driver panic: {panic:?}").into()),
    }
}

#[test]
fn structural_query_typed_holes_return_role_tag_scoped_matches() -> TestResult {
    let dir = tempfile::tempdir()?;
    let state_root = dir.path();
    let (socket, ingest_socket, shutdown, join) =
        start_runtime(state_root, "searchd-structural-typed-hole-success-test")?;
    publish_structural_ready_fixture(&ingest_socket)?;
    seal_lexical(&ingest_socket)?;
    seal_structural(&ingest_socket)?;

    let pin = GenerationPin::new(repo(), revision(), generation());
    let expr_request = SearchPlaneQueryIpcRequestEnvelope {
        request_id: 48,
        payload: SearchPlaneQueryIpcRequest::Structural(StructuralQueryRequest {
            text_query: TextQueryRequest {
                syntax: TextQuerySyntax::Native,
                query_text: "match { function_item { { :[name.expr] } } }".to_string(),
                constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                generation: Some(pin.clone()),
                generation_selector: None,
                top_k: 50,
            },
        }),
    };
    let mut observed_expr: Option<String> = None;
    let saw_expr = wait_until(READINESS_TIMEOUT, || {
        match send_query_request(&socket, &expr_request) {
            Ok(response) => match response.payload {
                SearchPlaneQueryIpcResponse::Structural(structural) => {
                    observed_expr = Some(format!("{structural:?}"));
                    structural.generation == pin && structural.results.len() == 1
                }
                SearchPlaneQueryIpcResponse::Error(err)
                    if err.code == "NOT_READY" || err.code == "STR_GENERATION_NOT_READY" =>
                {
                    observed_expr = Some(err.code);
                    false
                }
                other => {
                    observed_expr = Some(format!("{other:?}"));
                    false
                }
            },
            Err(err) => {
                observed_expr = Some(err.to_string());
                false
            }
        }
    });
    if !saw_expr {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!(
            "typed expr structural query never became ready; observed {observed_expr:?}"
        )
        .into());
    }

    let expr_response = send_query_request(&socket, &expr_request)?;
    let expr_structural = match expr_response.payload {
        SearchPlaneQueryIpcResponse::Structural(structural) => structural,
        other => {
            shutdown.store(true, Ordering::Release);
            drop(join.join());
            return Err(format!("expected Structural for typed expr, got {other:?}").into());
        }
    };
    let expr_candidate = expr_structural
        .results
        .first()
        .ok_or_else(|| "missing typed expr candidate".to_string())?;
    let expr_binding = expr_candidate
        .bindings
        .first()
        .ok_or_else(|| "missing typed expr binding".to_string())?;
    if expr_candidate.candidate_id != "chunk-tree"
        || expr_binding.metavariable != "name"
        || expr_binding.start_byte != 3
        || expr_binding.end_byte != 7
    {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!("unexpected typed expr candidate/binding: {expr_candidate:?}").into());
    }

    let item_response = send_query_request(
        &socket,
        &SearchPlaneQueryIpcRequestEnvelope {
            request_id: 49,
            payload: SearchPlaneQueryIpcRequest::Structural(StructuralQueryRequest {
                text_query: TextQueryRequest {
                    syntax: TextQuerySyntax::Native,
                    query_text: "match { :[root.item] }".to_string(),
                    constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                    generation: Some(pin),
                    generation_selector: None,
                    top_k: 50,
                },
            }),
        },
    )?;
    let item_structural = match item_response.payload {
        SearchPlaneQueryIpcResponse::Structural(structural) => structural,
        other => {
            shutdown.store(true, Ordering::Release);
            drop(join.join());
            return Err(format!("expected Structural for typed item, got {other:?}").into());
        }
    };
    let item_candidate = item_structural
        .results
        .first()
        .ok_or_else(|| "missing typed item candidate".to_string())?;
    let item_binding = item_candidate
        .bindings
        .first()
        .ok_or_else(|| "missing typed item binding".to_string())?;
    if item_candidate.candidate_id != "chunk-tree"
        || item_binding.metavariable != "root"
        || item_binding.start_byte != 0
        || item_binding.end_byte != 10
    {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(format!("unexpected typed item candidate/binding: {item_candidate:?}").into());
    }

    stop_runtime(shutdown, join)
}

#[test]
fn structural_query_rejects_typed_hole_kind_with_exact_code() -> TestResult {
    let dir = tempfile::tempdir()?;
    let state_root = dir.path();
    let (socket, _ingest_socket, shutdown, join) =
        start_runtime(state_root, "searchd-structural-typed-hole-reject-test")?;

    let response = send_query_request(
        &socket,
        &SearchPlaneQueryIpcRequestEnvelope {
            request_id: 49,
            payload: SearchPlaneQueryIpcRequest::Structural(StructuralQueryRequest {
                text_query: TextQueryRequest {
                    syntax: TextQuerySyntax::Native,
                    query_text: "match { function_item { { :[name.lambda] } } }".to_string(),
                    constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                    generation: Some(GenerationPin::new(repo(), revision(), generation())),
                    generation_selector: None,
                    top_k: 50,
                },
            }),
        },
    )?;
    let err = match response.payload {
        SearchPlaneQueryIpcResponse::Error(err) => err,
        other => {
            shutdown.store(true, Ordering::Release);
            drop(join.join());
            return Err(format!("expected Error, got {other:?}").into());
        }
    };
    if err.code != "STR_HOLE_KIND_UNSUPPORTED" || !err.message.contains("typed hole kind `lambda`")
    {
        shutdown.store(true, Ordering::Release);
        drop(join.join());
        return Err(
            format!("expected STR_HOLE_KIND_UNSUPPORTED typed-hole error, got {err:?}").into(),
        );
    }

    stop_runtime(shutdown, join)
}

fn lex_query(needle: &str) -> SearchPlaneQueryIpcRequestEnvelope {
    SearchPlaneQueryIpcRequestEnvelope {
        request_id: 0,
        payload: SearchPlaneQueryIpcRequest::Text(TextQueryRequest {
            syntax: TextQuerySyntax::Sourcegraph,
            query_text: needle.to_string(),
            constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
            generation: Some(GenerationPin::new(repo(), revision(), generation())),
            generation_selector: None,
            top_k: 50,
        }),
    }
}

fn history_query(query_text: &str) -> SearchPlaneQueryIpcRequestEnvelope {
    history_query_with_syntax(TextQuerySyntax::Sourcegraph, query_text)
}

fn history_query_with_syntax(
    syntax: TextQuerySyntax,
    query_text: &str,
) -> SearchPlaneQueryIpcRequestEnvelope {
    SearchPlaneQueryIpcRequestEnvelope {
        request_id: 0,
        payload: SearchPlaneQueryIpcRequest::History(HistoryQueryRequest {
            text_query: TextQueryRequest {
                syntax,
                query_text: query_text.to_string(),
                constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                generation: Some(GenerationPin::new(repo(), revision(), generation())),
                generation_selector: None,
                top_k: 50,
            },
            cursor: None,
        }),
    }
}

fn runtime_metadata_query(query_text: &str) -> SearchPlaneQueryIpcRequestEnvelope {
    runtime_metadata_query_with_syntax(TextQuerySyntax::Sourcegraph, query_text)
}

fn runtime_metadata_query_with_syntax(
    syntax: TextQuerySyntax,
    query_text: &str,
) -> SearchPlaneQueryIpcRequestEnvelope {
    SearchPlaneQueryIpcRequestEnvelope {
        request_id: 0,
        payload: SearchPlaneQueryIpcRequest::RuntimeMetadata(RuntimeMetadataQueryRequest {
            text_query: TextQueryRequest {
                syntax,
                query_text: query_text.to_string(),
                constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                generation: Some(GenerationPin::new(repo(), revision(), generation())),
                generation_selector: None,
                top_k: 50,
            },
        }),
    }
}

fn wait_for_typed_error(
    socket: &Path,
    request: &SearchPlaneQueryIpcRequestEnvelope,
    timeout: Duration,
) -> Result<quanta_index_contract::SearchPlaneIpcError, Box<dyn Error>> {
    let mut last_observed = String::new();
    if !wait_until(timeout, || match send_query_request(socket, request) {
        Ok(response) => match response.payload {
            SearchPlaneQueryIpcResponse::Error(err) => {
                last_observed = err.code;
                true
            }
            other => {
                last_observed = format!("{other:?}");
                false
            }
        },
        Err(err) => {
            last_observed = err.to_string();
            false
        }
    }) {
        return Err(format!("typed error never surfaced before timeout: {last_observed}").into());
    }
    let response = send_query_request(socket, request)?;
    match response.payload {
        SearchPlaneQueryIpcResponse::Error(err) => Ok(err),
        other => Err(format!("expected Error response after readiness wait, got {other:?}").into()),
    }
}
