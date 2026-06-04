//! SDK-frontdoor end-to-end proof for source-authority packets.

#![forbid(unsafe_code)]
#![expect(
    clippy::disallowed_methods,
    reason = "integration polling uses explicit Result fallback checks"
)]

#[path = "common/frontdoor_scenarios.rs"]
mod frontdoor_scenarios;

use std::collections::BTreeSet;
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
    ChunkId, ChunkRecord, GenerationPin, GenerationSelector, HistoryQueryRequest,
    HybridQueryRequest, ManifestGeneration, RepoId, RepoMapActivateGenerationRequest,
    RepoMapChunkExactness, RepoMapChunkNode, RepoMapContainsEdge, RepoMapDocType, RepoMapEdge,
    RepoMapExactnessSummary, RepoMapFileNode, RepoMapFocusSubjectDto, RepoMapGraphCoverage,
    RepoMapGraphCoverageClass, RepoMapItemIndexAvailability, RepoMapNode, RepoMapNodeRef,
    RepoMapOwnsChunkEdge, RepoMapQueryRequest, RepoMapRedactionState, RepoMapSourceBundle,
    RepoMapSymbolNode, RevisionId, RuntimeCatalogIngestBatch, RuntimeChangedRecord,
    RuntimeDocFacetRecord, RuntimeEdgeAuthorityRecord, RuntimeMetadataQueryRequest,
    RuntimeSnapshotRecord, SearchPlaneActivateGenerationRequest, SearchPlaneIngestIpcRequest,
    SearchPlaneIngestIpcRequestEnvelope, SearchPlaneIngestIpcResponse,
    SearchPlaneIngestIpcResponseEnvelope, SearchPlaneTrackKind, SemanticQueryRequest,
    StructuralQueryRequest, SymbolId, SymbolQueryRequest, TextQueryRequest, TextQuerySyntax,
};
use quanta_index_ipc::send_request;
use quanta_index_sdk::{
    CommitRecord, CommitSha, ConnectOptions, DiffHunkRecord, DirtyBatch, DirtyRecord, LexicalBatch,
    ParseNode, ParseRoleTag, ParseTreeRecord, QuantaIndex, RepoRelativePath, SdkError,
    SearchScopeKey, SearchScopeSurface, StructuralBatch,
};
use quanta_index_searchd::app::SearchdConfig;
use quanta_index_searchd::app::searchd::drive;
use quanta_index_searchd_runtime::build_runtime;

use crate::frontdoor_scenarios::{
    SDK_FRONTDOOR_SCENARIOS, SdkFrontdoorExpectation, SdkFrontdoorSurface,
};

type TestResult = Result<(), Box<dyn Error>>;
type DriverJoin = thread::JoinHandle<anyhow::Result<()>>;
type SdkFrontdoorRuntime = (tempfile::TempDir, QuantaIndex, Arc<AtomicBool>, DriverJoin);
type SdkFrontdoorRuntimeWithIngest = (
    tempfile::TempDir,
    QuantaIndex,
    PathBuf,
    Arc<AtomicBool>,
    DriverJoin,
);

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

fn generation_two() -> ManifestGeneration {
    ManifestGeneration::new(32)
}

fn pin_two() -> GenerationPin {
    GenerationPin::new(repo(), revision(), generation_two())
}

fn active_selector() -> GenerationSelector {
    GenerationSelector::Active {
        repo_id: repo(),
        revision_id: revision(),
    }
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

fn start_sdk_frontdoor_runtime_at_state_root(
    state_root: &Path,
    thread_name: &str,
) -> Result<(QuantaIndex, Arc<AtomicBool>, DriverJoin), Box<dyn Error>> {
    let runtime = build_runtime(build_config(state_root))?;
    let query_socket = runtime.query_server.socket_path().to_path_buf();
    let control_socket = runtime.control_server.socket_path().to_path_buf();
    let ingest_socket = runtime.ingest_server.socket_path().to_path_buf();
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_for_drive = Arc::clone(&shutdown);
    let join = thread::Builder::new()
        .name(thread_name.into())
        .spawn(move || drive(runtime, &shutdown_for_drive))?;

    if !wait_until(SOCKET_TIMEOUT, || {
        query_socket.exists() && control_socket.exists() && ingest_socket.exists()
    }) {
        stop_runtime(&shutdown, join)?;
        return Err("sdk frontdoor sockets never appeared".into());
    }

    let client = match QuantaIndex::connect(
        ConnectOptions::from_state_root(state_root)
            .with_query_socket(query_socket)
            .with_control_socket(control_socket)
            .with_ingest_socket(ingest_socket),
    ) {
        Ok(client) => client,
        Err(err) => {
            stop_runtime(&shutdown, join)?;
            return Err(format!("sdk frontdoor connect failed: {err}").into());
        }
    };

    Ok((client, shutdown, join))
}

fn start_sdk_frontdoor_runtime(thread_name: &str) -> Result<SdkFrontdoorRuntime, Box<dyn Error>> {
    let dir = tempfile::tempdir()?;
    let (client, shutdown, join) =
        start_sdk_frontdoor_runtime_at_state_root(dir.path(), thread_name)?;

    Ok((dir, client, shutdown, join))
}

fn start_sdk_frontdoor_runtime_with_ingest(
    thread_name: &str,
) -> Result<SdkFrontdoorRuntimeWithIngest, Box<dyn Error>> {
    let dir = tempfile::tempdir()?;
    let runtime = build_runtime(build_config(dir.path()))?;
    let query_socket = runtime.query_server.socket_path().to_path_buf();
    let control_socket = runtime.control_server.socket_path().to_path_buf();
    let ingest_socket = runtime.ingest_server.socket_path().to_path_buf();
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_for_drive = Arc::clone(&shutdown);
    let join = thread::Builder::new()
        .name(thread_name.into())
        .spawn(move || drive(runtime, &shutdown_for_drive))?;

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
            .with_ingest_socket(ingest_socket.clone()),
    )?;
    Ok((dir, client, ingest_socket, shutdown, join))
}

fn dispatch_ingest(socket: &Path, payload: SearchPlaneIngestIpcRequest) -> TestResult {
    let response: SearchPlaneIngestIpcResponseEnvelope = send_request(
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
        | SearchPlaneIngestIpcResponse::RuntimeCatalogReceipt(_)
        | SearchPlaneIngestIpcResponse::StructuralReceipt(_)
        | SearchPlaneIngestIpcResponse::RepoMapReceipt(_) => Ok(()),
        SearchPlaneIngestIpcResponse::Error(err) => {
            Err(format!("ingest failed code={} message={}", err.code, err.message).into())
        }
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

fn history_commit_only_batch() -> quanta_index_sdk::HistoryBatch {
    quanta_index_sdk::HistoryBatch::new(
        repo(),
        revision(),
        generation(),
        "batch:history-sdk-commit-only",
    )
    .manifest_digest("manifest:history-sdk-commit-only")
    .commit(CommitRecord {
        wire_version: 1,
        sha: commit_sha(),
        parents: Vec::new(),
        author_time_ms: 21,
        committer_time_ms: 22,
        applied_at_ms: 23,
        author: "alice".to_string().into_boxed_str(),
        committer: "alice".to_string().into_boxed_str(),
        message: "todo: shard gap".to_string().into_boxed_str(),
        is_merge: false,
        tags: Vec::new(),
    })
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
        "scope:lexical-lib",
        vec![
            lexical_chunk(
                "chunk-dirty",
                "src/lib.rs",
                "todo!()",
                "text:digest",
                "shape:digest",
                12,
            )?,
            lexical_chunk(
                "chunk-tree",
                "src/lib.rs",
                "fn main() {}",
                "text:tree",
                "shape:tree",
                12,
            )?,
        ],
        vec![symbol_record()?],
    )
    .replace_scope(
        SearchScopeKey {
            doc_surface: SearchScopeSurface::File,
            repo_relative_path: RepoRelativePath::new("src/alpha.rs"),
        },
        "scope:lexical-alpha",
        vec![lexical_chunk(
            "alpha",
            "src/alpha.rs",
            "sphinx of quartz",
            "text:alpha",
            "shape:alpha",
            16,
        )?],
        Vec::new(),
    )
    .replace_scope(
        SearchScopeKey {
            doc_surface: SearchScopeSurface::File,
            repo_relative_path: RepoRelativePath::new("src/beta.rs"),
        },
        "scope:lexical-beta",
        vec![lexical_chunk(
            "beta",
            "src/beta.rs",
            "sphinx riddles",
            "text:beta",
            "shape:beta",
            14,
        )?],
        Vec::new(),
    ))
}

fn lexical_frontdoor_matrix_batch() -> Result<LexicalBatch, Box<dyn Error>> {
    Ok(lexical_batch()?.replace_scope(
        SearchScopeKey {
            doc_surface: SearchScopeSurface::File,
            repo_relative_path: RepoRelativePath::new("src/file_contains.rs"),
        },
        "scope:lexical-file-contains",
        vec![lexical_chunk(
            "chunk-file-contains",
            "src/file_contains.rs",
            "foo oo_ba file_contains_needle",
            "text:file-contains",
            "shape:file-contains",
            29,
        )?],
        Vec::new(),
    ))
}

fn lexical_batch_two() -> Result<LexicalBatch, Box<dyn Error>> {
    Ok(LexicalBatch::replace_generation(
        repo(),
        revision(),
        generation_two(),
        "manifest:lexical-v2",
        "batch:lexical-v2",
    )
    .replace_scope(
        SearchScopeKey {
            doc_surface: SearchScopeSurface::File,
            repo_relative_path: RepoRelativePath::new("src/lib.rs"),
        },
        "scope:lexical-lib-v2",
        vec![
            lexical_chunk(
                "chunk-dirty-v2",
                "src/lib.rs",
                "todo_v2!()",
                "text:digest:v2",
                "shape:digest:v2",
                15,
            )?,
            lexical_chunk(
                "chunk-tree-v2",
                "src/lib.rs",
                "fn upgraded() {}",
                "text:tree:v2",
                "shape:tree:v2",
                16,
            )?,
        ],
        vec![symbol_record()?],
    )
    .replace_scope(
        SearchScopeKey {
            doc_surface: SearchScopeSurface::File,
            repo_relative_path: RepoRelativePath::new("src/gamma.rs"),
        },
        "scope:lexical-gamma",
        vec![lexical_chunk(
            "gamma",
            "src/gamma.rs",
            "obsidian gamma",
            "text:gamma",
            "shape:gamma",
            14,
        )?],
        Vec::new(),
    ))
}

fn lexical_chunk(
    chunk_id: &str,
    path: &str,
    snippet: &str,
    _text_digest: &str,
    _shape_digest: &str,
    end_byte: u32,
) -> Result<ChunkRecord, Box<dyn Error>> {
    Ok(ChunkRecord {
        chunk_id: ChunkId::new(chunk_id),
        repo_relative_path: RepoRelativePath::new(path),
        language: rust_language()?,
        start_byte: 0,
        end_byte,
        start_line: 1,
        end_line: 1,
        text: snippet.to_string().into_boxed_str(),
        structural: None,
        parent_chunk_id: None,
        source_repo_id: None,
    })
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

fn repo_map_bundle() -> Result<RepoMapSourceBundle, Box<dyn Error>> {
    let symbol_kind = SymbolKindCode::new("struct").map_err(|err| -> Box<dyn Error> {
        format!("invalid repo-map symbol kind: {err}").into()
    })?;
    Ok(RepoMapSourceBundle::new(
        repo(),
        revision(),
        generation(),
        "manifest-digest-sdk",
        "repomap-snapshot-sdk",
        1,
        "e".repeat(64),
        RepoMapGraphCoverage {
            item_index_availability: RepoMapItemIndexAvailability::Available,
            graph_coverage_class: RepoMapGraphCoverageClass::Complete,
        },
        RepoMapExactnessSummary::Exact,
        RepoMapRedactionState::Unredacted,
    )
    .with_node(RepoMapNode::File(RepoMapFileNode {
        file_id: quanta_index_contract::FileId::new("file://src/lib.rs"),
        repo_relative_path: RepoRelativePath::new("src/lib.rs"),
        line_count: 110,
    }))
    .with_node(RepoMapNode::File(RepoMapFileNode {
        file_id: quanta_index_contract::FileId::new("file://src/service/mod.rs"),
        repo_relative_path: RepoRelativePath::new("src/service/mod.rs"),
        line_count: 170,
    }))
    .with_node(RepoMapNode::File(RepoMapFileNode {
        file_id: quanta_index_contract::FileId::new("file://tests/repo_map.rs"),
        repo_relative_path: RepoRelativePath::new("tests/repo_map.rs"),
        line_count: 70,
    }))
    .with_node(RepoMapNode::Symbol(RepoMapSymbolNode {
        symbol_id: SymbolId::new("symbol://alpha"),
        owner_path: RepoRelativePath::new("src/lib.rs"),
        local_name: "Alpha".to_string(),
        qualified_name: "src::lib::Alpha".to_string(),
        symbol_kind: symbol_kind.clone(),
    }))
    .with_node(RepoMapNode::Symbol(RepoMapSymbolNode {
        symbol_id: SymbolId::new("symbol://beta"),
        owner_path: RepoRelativePath::new("src/service/mod.rs"),
        local_name: "Beta".to_string(),
        qualified_name: "src::service::Beta".to_string(),
        symbol_kind,
    }))
    .with_node(RepoMapNode::Chunk(RepoMapChunkNode {
        chunk_id: ChunkId::new("chunk://alpha"),
        owner_path: RepoRelativePath::new("src/lib.rs"),
        language: rust_language()?,
        start_byte: 0,
        end_byte: 128,
        start_line: 1,
        end_line: 12,
        token_count: 64,
        preview_text: "Alpha library owner index".to_string(),
        exactness: RepoMapChunkExactness::Exact,
    }))
    .with_node(RepoMapNode::Chunk(RepoMapChunkNode {
        chunk_id: ChunkId::new("chunk://beta"),
        owner_path: RepoRelativePath::new("src/service/mod.rs"),
        language: rust_language()?,
        start_byte: 129,
        end_byte: 256,
        start_line: 13,
        end_line: 28,
        token_count: 96,
        preview_text: "Beta service owner query entrypoint".to_string(),
        exactness: RepoMapChunkExactness::Exact,
    }))
    .with_node(RepoMapNode::Chunk(RepoMapChunkNode {
        chunk_id: ChunkId::new("chunk://repomap-test"),
        owner_path: RepoRelativePath::new("tests/repo_map.rs"),
        language: rust_language()?,
        start_byte: 257,
        end_byte: 320,
        start_line: 29,
        end_line: 35,
        token_count: 40,
        preview_text: "repo map integration test".to_string(),
        exactness: RepoMapChunkExactness::Approximate,
    }))
    .with_edge(RepoMapEdge::Contains(RepoMapContainsEdge {
        container: RepoMapNodeRef::File(quanta_index_contract::FileId::new("file://src/lib.rs")),
        contained: RepoMapNodeRef::Symbol(SymbolId::new("symbol://alpha")),
    }))
    .with_edge(RepoMapEdge::Contains(RepoMapContainsEdge {
        container: RepoMapNodeRef::File(quanta_index_contract::FileId::new(
            "file://src/service/mod.rs",
        )),
        contained: RepoMapNodeRef::Symbol(SymbolId::new("symbol://beta")),
    }))
    .with_edge(RepoMapEdge::Call(quanta_index_contract::RepoMapCallEdge {
        caller: RepoMapNodeRef::Symbol(SymbolId::new("symbol://beta")),
        callee: RepoMapNodeRef::Symbol(SymbolId::new("symbol://alpha")),
    }))
    .with_edge(RepoMapEdge::Call(quanta_index_contract::RepoMapCallEdge {
        caller: RepoMapNodeRef::Symbol(SymbolId::new("symbol://beta")),
        callee: RepoMapNodeRef::File(quanta_index_contract::FileId::new(
            "file://tests/repo_map.rs",
        )),
    }))
    .with_edge(RepoMapEdge::Import(
        quanta_index_contract::RepoMapImportEdge {
            importer: RepoMapNodeRef::File(quanta_index_contract::FileId::new(
                "file://src/service/mod.rs",
            )),
            imported: RepoMapNodeRef::File(quanta_index_contract::FileId::new("file://src/lib.rs")),
        },
    ))
    .with_edge(RepoMapEdge::OwnsChunk(RepoMapOwnsChunkEdge {
        owner: RepoMapNodeRef::Symbol(SymbolId::new("symbol://alpha")),
        chunk: RepoMapNodeRef::Chunk(ChunkId::new("chunk://alpha")),
    }))
    .with_edge(RepoMapEdge::OwnsChunk(RepoMapOwnsChunkEdge {
        owner: RepoMapNodeRef::Symbol(SymbolId::new("symbol://beta")),
        chunk: RepoMapNodeRef::Chunk(ChunkId::new("chunk://beta")),
    }))
    .with_edge(RepoMapEdge::OwnsChunk(RepoMapOwnsChunkEdge {
        owner: RepoMapNodeRef::File(quanta_index_contract::FileId::new(
            "file://tests/repo_map.rs",
        )),
        chunk: RepoMapNodeRef::Chunk(ChunkId::new("chunk://repomap-test")),
    })))
}

fn repo_map_activate_request() -> RepoMapActivateGenerationRequest {
    RepoMapActivateGenerationRequest {
        repo_id: repo(),
        revision_id: revision(),
        manifest_generation: generation(),
        manifest_digest: "manifest-digest-sdk".to_string(),
    }
}

fn repo_map_query_request() -> RepoMapQueryRequest {
    RepoMapQueryRequest {
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
    }
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
            role_tags: structural_role_tags(10, 3, 7, 8, 10),
        },
    ))
}

fn structural_batch() -> Result<StructuralBatch, Box<dyn Error>> {
    structural_batch_with_chunk(ChunkId::new("chunk-tree"))
}

fn structural_batch_two() -> Result<StructuralBatch, Box<dyn Error>> {
    Ok(StructuralBatch::replace_generation(
        repo(),
        revision(),
        generation_two(),
        "manifest:structural-v2",
        "batch:structural-v2",
    )
    .replace_tree(
        structural_scope(),
        "scope:structural-v2",
        ChunkId::new("chunk-tree-v2"),
        ParseTreeRecord {
            wire_version: 1,
            lang: rust_language()?,
            root: ParseNode {
                kind: "function_item".to_string().into_boxed_str(),
                byte_start: 0,
                byte_end: 14,
                children: vec![
                    ParseNode {
                        kind: "identifier".to_string().into_boxed_str(),
                        byte_start: 3,
                        byte_end: 11,
                        children: Vec::new(),
                    },
                    ParseNode {
                        kind: "block".to_string().into_boxed_str(),
                        byte_start: 12,
                        byte_end: 14,
                        children: Vec::new(),
                    },
                ],
            },
            source_hash: compute_parse_tree_source_hash("fn upgraded() {}"),
            role_tag_schema_version: 1,
            role_tags: structural_role_tags(14, 3, 11, 12, 14),
        },
    ))
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

fn rust_language() -> Result<LanguageCode, Box<dyn Error>> {
    LanguageCode::new("rust").map_err(|err| -> Box<dyn Error> {
        format!("invalid hard-coded test language code: {err}").into()
    })
}

fn publish_runtime_catalog_batch(socket: &Path) -> TestResult {
    dispatch_ingest(
        socket,
        SearchPlaneIngestIpcRequest::PublishRuntimeCatalogBatch(RuntimeCatalogIngestBatch {
            repo_id: repo(),
            revision_id: revision(),
            generation: generation(),
            overlay_epoch_ms: 20,
            batch_digest: "batch:runtime-catalog-sdk".to_string(),
            producer_head_applied_at_ms: 100,
            generation_materialized_at_ms: 20,
            changed_entries: vec![RuntimeChangedRecord {
                doc_id: ChunkId::new("alpha"),
                applied_at_ms: 25,
                payload_hash: [0xaa; 32],
            }],
            facet_entries: vec![RuntimeDocFacetRecord {
                doc_id: ChunkId::new("alpha"),
                owner: Some("team-a".to_string()),
                service: Some("search".to_string()),
                layer: Some("index".to_string()),
                surface: Some("lexical".to_string()),
            }],
            snapshot_entries: vec![RuntimeSnapshotRecord {
                name: "active".to_string(),
                doc_ids: vec![ChunkId::new("alpha")],
            }],
            affected_entries: vec![RuntimeEdgeAuthorityRecord {
                key: "rebuild=lexical".to_string(),
                doc_ids: vec![ChunkId::new("alpha")],
            }],
            invalidated_by_entries: vec![RuntimeEdgeAuthorityRecord {
                key: "rebuild=lexical".to_string(),
                doc_ids: vec![ChunkId::new("alpha")],
            }],
        }),
    )
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

fn expect_sdk_error<T>(
    result: Result<T, SdkError>,
    context: &str,
) -> Result<SdkError, Box<dyn Error>> {
    match result {
        Ok(_) => Err(format!("{context}: unexpectedly succeeded").into()),
        Err(err) => Ok(err),
    }
}

fn publish_sdk_lexical_and_structural_ready(client: &QuantaIndex) -> TestResult {
    let _lexical_receipt = client.lexical().publish(&lexical_batch()?)?;
    let _structural_receipt = client.structural().publish(&structural_batch()?)?;
    let _activation = wait_for_sdk_ready(SOCKET_TIMEOUT, || {
        client
            .generations()
            .activate()
            .repo(repo())
            .revision(revision())
            .generation(generation())
            .manifest_digest("manifest:sdk-structural-ready")
            .tracks([
                SearchPlaneTrackKind::Lexical,
                SearchPlaneTrackKind::Structural,
            ])?
            .commit()
    })?;
    Ok(())
}

fn assert_structural_single_binding(
    response: &quanta_index_contract::SearchPlaneStructuralQueryResponse,
    expected_generation: &GenerationPin,
    expected_candidate_id: &str,
    expected_metavariable: &str,
    expected_start_byte: u32,
    expected_end_byte: u32,
    context: &str,
) -> TestResult {
    if response.generation != *expected_generation || response.results.len() != 1 {
        return Err(format!("{context}: unexpected structural response: {response:?}").into());
    }
    let candidate = response
        .results
        .first()
        .ok_or_else(|| format!("{context}: missing structural candidate"))?;
    let binding = candidate
        .bindings
        .first()
        .ok_or_else(|| format!("{context}: missing structural binding"))?;
    if candidate.candidate_id != expected_candidate_id
        || binding.metavariable != expected_metavariable
        || binding.start_byte != expected_start_byte
        || binding.end_byte != expected_end_byte
    {
        return Err(
            format!("{context}: unexpected structural candidate/binding: {candidate:?}").into(),
        );
    }
    Ok(())
}

fn expect_usage_error_contains<T>(
    result: Result<T, SdkError>,
    expected_fragment: &str,
) -> TestResult {
    match result {
        Err(SdkError::Usage(message)) if message.contains(expected_fragment) => Ok(()),
        Err(other) => Err(format!(
            "expected usage error containing {expected_fragment:?}, got {other:?}"
        )
        .into()),
        Ok(_) => Err(
            format!("expected usage error containing {expected_fragment:?}, got success").into(),
        ),
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

fn wait_for_sdk_observation<T, F, P>(timeout: Duration, run: F, ready: P) -> Result<T, SdkError>
where
    F: FnMut() -> Result<T, SdkError>,
    P: FnMut(&T) -> bool,
{
    wait_for_sdk_observation_with_retry_codes(timeout, &["NOT_READY"], run, ready)
}

fn wait_for_sdk_observation_with_retry_codes<T, F, P>(
    timeout: Duration,
    retry_codes: &[&str],
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
                if retry_codes.iter().any(|candidate| code == *candidate)
                    && start.elapsed() < timeout =>
            {
                drop(message);
                thread::sleep(Duration::from_millis(10));
            }
            Err(err) => return Err(err),
        }
    }
}

fn wait_for_sdk_terminal_error<T, F>(
    timeout: Duration,
    retry_codes: &[&str],
    mut run: F,
) -> Result<SdkError, Box<dyn Error>>
where
    F: FnMut() -> Result<T, SdkError>,
{
    let start = Instant::now();
    loop {
        match run() {
            Ok(_) => {
                return Err("query unexpectedly succeeded while waiting for typed error".into());
            }
            Err(SdkError::Remote { code, message })
                if retry_codes.iter().any(|candidate| code == *candidate)
                    && start.elapsed() < timeout =>
            {
                drop(message);
                thread::sleep(Duration::from_millis(10));
            }
            Err(err) => return Ok(err),
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

fn assert_repo_map_happy_path(
    response: &quanta_index_contract::RepoMapQueryResponse,
) -> TestResult {
    if response.repo_id != repo()
        || response.revision_id != revision()
        || response.manifest_generation != generation()
        || response.snapshot_meta.snapshot_id != "repomap-snapshot-sdk"
    {
        return Err(format!("unexpected repo-map response envelope: {response:?}").into());
    }
    let entry = response
        .entries
        .first()
        .ok_or_else(|| "missing repo-map entry".to_string())?;
    if entry.subject_identity != "symbol://beta"
        || entry.owner_path != "src/service/mod.rs"
        || entry.subject_doc_type != RepoMapDocType::Symbol
        || entry.projection_evidence_kind != "AuthorityBundle"
    {
        return Err(format!("unexpected repo-map entry: {entry:?}").into());
    }
    if response
        .entries
        .iter()
        .filter(|entry| entry.included)
        .count()
        != 1
    {
        return Err(format!("unexpected repo-map inclusion set: {response:?}").into());
    }
    if !response
        .degraded_reason_codes
        .iter()
        .any(|code| code == "token_budget_floor_applied")
    {
        return Err(format!("missing repo-map degraded reason: {response:?}").into());
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
fn sdk_publish_frontdoor_routes_ingest_batches() -> TestResult {
    let dir = tempfile::tempdir()?;
    let runtime = build_runtime(build_config(dir.path()))?;
    let query_socket = runtime.query_server.socket_path().to_path_buf();
    let control_socket = runtime.control_server.socket_path().to_path_buf();
    let ingest_socket = runtime.ingest_server.socket_path().to_path_buf();
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_for_drive = Arc::clone(&shutdown);
    let join = thread::Builder::new()
        .name("sdk-frontdoor-publish".into())
        .spawn(move || drive(runtime, &shutdown_for_drive))?;

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
    let repo_map_receipt = client.repomap().publish(&repo_map_bundle()?)?;

    if lexical_receipt.generation != generation()
        || lexical_receipt.accepted_replace_scopes != 3
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
    if repo_map_receipt.repo_id != repo()
        || repo_map_receipt.revision_id != revision()
        || repo_map_receipt.manifest_generation != generation()
    {
        stop_runtime(&shutdown, join)?;
        return Err(format!("unexpected repo-map receipt: {repo_map_receipt:?}").into());
    }

    stop_runtime(&shutdown, join)
}

#[test]
fn sdk_search_frontdoor_routes_lexical_semantic_hybrid_explain_and_repomap_truth() -> TestResult {
    let dir = tempfile::tempdir()?;
    let runtime = build_runtime(build_config(dir.path()))?;
    let query_socket = runtime.query_server.socket_path().to_path_buf();
    let control_socket = runtime.control_server.socket_path().to_path_buf();
    let ingest_socket = runtime.ingest_server.socket_path().to_path_buf();
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_for_drive = Arc::clone(&shutdown);
    let join = thread::Builder::new()
        .name("sdk-frontdoor-search".into())
        .spawn(move || drive(runtime, &shutdown_for_drive))?;

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
    let repo_map_receipt = client.repomap().publish(&repo_map_bundle()?)?;
    if repo_map_receipt.manifest_generation != generation() {
        stop_runtime(&shutdown, join)?;
        return Err(format!("unexpected repo-map publish ack: {repo_map_receipt:?}").into());
    }

    let _activation = wait_for_sdk_ready(SOCKET_TIMEOUT, || {
        client
            .generations()
            .activate()
            .repo(repo())
            .revision(revision())
            .generation(generation())
            .manifest_digest("manifest:lexical")
            .tracks([
                SearchPlaneTrackKind::Lexical,
                SearchPlaneTrackKind::Semantic,
            ])?
            .commit()
    })?;
    let repo_map_activation = client.repomap().activate(repo_map_activate_request())?;
    if repo_map_activation.manifest_generation != generation() {
        stop_runtime(&shutdown, join)?;
        return Err(format!("unexpected repo-map activate ack: {repo_map_activation:?}").into());
    }

    let lexical = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .lexical()
                .query()
                .sourcegraph("todo")
                .active(repo(), revision())
                .top_k(2)
                .execute()
        },
        |response| response.generation == pin() && response.results.len() == 1,
    )?;
    let lexical_candidate = lexical
        .results
        .first()
        .cloned()
        .ok_or_else(|| "missing lexical candidate".to_string())?;
    if lexical.generation != pin()
        || lexical_candidate.candidate_id != "chunk-dirty"
        || lexical_candidate.repo_relative_path.as_str() != "src/lib.rs"
    {
        stop_runtime(&shutdown, join)?;
        return Err(format!("unexpected lexical response: {lexical:?}").into());
    }

    let lexical_select_path = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .lexical()
                .query()
                .sourcegraph("select:path sphinx")
                .active(repo(), revision())
                .top_k(5)
                .execute()
        },
        |response| response.generation == pin() && response.results.len() == 2,
    )?;
    let lexical_select_path_paths = lexical_select_path
        .results
        .iter()
        .map(|candidate| candidate.repo_relative_path.as_str().to_string())
        .collect::<BTreeSet<_>>();
    if lexical_select_path.generation != pin()
        || lexical_select_path_paths
            != BTreeSet::from(["src/alpha.rs".to_string(), "src/beta.rs".to_string()])
    {
        stop_runtime(&shutdown, join)?;
        return Err(
            format!("unexpected select:path lexical response: {lexical_select_path:?}").into(),
        );
    }

    let lexical_select_content_match = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .lexical()
                .query()
                .sourcegraph("select:content.match sphinx")
                .active(repo(), revision())
                .top_k(5)
                .execute()
        },
        |response| response.generation == pin() && response.results.len() == 2,
    )?;
    let lexical_select_content_match_paths = lexical_select_content_match
        .results
        .iter()
        .map(|candidate| candidate.repo_relative_path.as_str().to_string())
        .collect::<BTreeSet<_>>();
    if lexical_select_content_match.generation != pin()
        || lexical_select_content_match_paths
            != BTreeSet::from(["src/alpha.rs".to_string(), "src/beta.rs".to_string()])
        || lexical_select_content_match
            .results
            .iter()
            .any(|candidate| !candidate.snippet.contains("sphinx"))
    {
        stop_runtime(&shutdown, join)?;
        return Err(format!(
            "unexpected select:content.match lexical response: {lexical_select_content_match:?}"
        )
        .into());
    }

    let lexical_native_select_path = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .lexical()
                .query()
                .native("select:path sphinx")
                .active(repo(), revision())
                .top_k(5)
                .execute()
        },
        |response| response.generation == pin() && response.results.len() == 2,
    )?;
    let lexical_native_select_path_paths = lexical_native_select_path
        .results
        .iter()
        .map(|candidate| candidate.repo_relative_path.as_str().to_string())
        .collect::<BTreeSet<_>>();
    if lexical_native_select_path.generation != pin()
        || lexical_native_select_path_paths
            != BTreeSet::from(["src/alpha.rs".to_string(), "src/beta.rs".to_string()])
    {
        stop_runtime(&shutdown, join)?;
        return Err(format!(
            "unexpected native select:path lexical response: {lexical_native_select_path:?}"
        )
        .into());
    }

    let lexical_native_select_content_match = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .lexical()
                .query()
                .native("select:content.match sphinx")
                .active(repo(), revision())
                .top_k(5)
                .execute()
        },
        |response| response.generation == pin() && response.results.len() == 2,
    )?;
    let lexical_native_select_content_match_paths = lexical_native_select_content_match
        .results
        .iter()
        .map(|candidate| candidate.repo_relative_path.as_str().to_string())
        .collect::<BTreeSet<_>>();
    if lexical_native_select_content_match.generation != pin()
        || lexical_native_select_content_match_paths
            != BTreeSet::from(["src/alpha.rs".to_string(), "src/beta.rs".to_string()])
        || lexical_native_select_content_match
            .results
            .iter()
            .any(|candidate| !candidate.snippet.contains("sphinx"))
    {
        stop_runtime(&shutdown, join)?;
        return Err(format!(
            "unexpected native select:content.match lexical response: \
             {lexical_native_select_content_match:?}"
        )
        .into());
    }

    let explain = wait_for_sdk_ready(SOCKET_TIMEOUT, || {
        client.search().explain(pin(), lexical_candidate.clone())
    })?;
    if explain.generation != pin()
        || !explain.explanation.summary.contains("present")
        || !explain.explanation.summary.contains("chunk-dirty")
    {
        stop_runtime(&shutdown, join)?;
        return Err(format!("unexpected explain response: {explain:?}").into());
    }

    let semantic = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .semantic()
                .query()
                .text("quartz")
                .active(repo(), revision())
                .top_k(2)
                .execute()
        },
        |response| response.generation == pin() && !response.results.is_empty(),
    )?;
    let semantic_top = semantic
        .results
        .first()
        .ok_or_else(|| "missing semantic candidate".to_string())?;
    if semantic.generation != pin()
        || semantic_top.candidate_id != "alpha"
        || semantic.explanation.summary.is_empty()
    {
        stop_runtime(&shutdown, join)?;
        return Err(format!("unexpected semantic response: {semantic:?}").into());
    }

    let hybrid = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .search()
                .hybrid()
                .sourcegraph("sphinx")
                .semantic_text("quartz")
                .active(repo(), revision())
                .top_k(2)
                .execute()
        },
        |response| response.generation == pin() && !response.results.is_empty(),
    )?;
    let hybrid_top = hybrid
        .results
        .first()
        .ok_or_else(|| "missing hybrid candidate".to_string())?;
    if hybrid.generation != pin()
        || hybrid_top.candidate_id != "alpha"
        || hybrid.explanation.summary.is_empty()
    {
        stop_runtime(&shutdown, join)?;
        return Err(format!("unexpected hybrid response: {hybrid:?}").into());
    }

    let repo_map = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || client.repomap().query(repo_map_query_request()),
        |response| response.manifest_generation == generation() && !response.entries.is_empty(),
    )?;
    assert_repo_map_happy_path(&repo_map)?;

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
        .spawn(move || drive(runtime, &shutdown_for_drive))?;

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
            ])?
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
                .pinned(pin())
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
                .pinned(pin())
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
                .pinned(pin())
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

    let structural_boolean_and = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .structural()
                .query()
                .native("match { function_item } AND match { function_item :[x] }")
                .pinned(pin())
                .top_k(2)
                .execute()
        },
        |response| response.generation == pin() && response.results.len() == 1,
    )?;
    let structural_boolean_and_candidate = structural_boolean_and
        .results
        .first()
        .ok_or_else(|| "missing structural boolean AND candidate".to_string())?;
    let structural_boolean_and_binding = structural_boolean_and_candidate
        .bindings
        .first()
        .ok_or_else(|| "missing structural boolean AND binding".to_string())?;
    if structural_boolean_and_candidate.candidate_id != "chunk-tree"
        || structural_boolean_and_binding.metavariable != "x"
        || structural_boolean_and_binding.start_byte != 0
        || structural_boolean_and_binding.end_byte != 10
    {
        stop_runtime(&shutdown, join)?;
        return Err(format!(
            "unexpected structural boolean AND candidate/binding: \
             {structural_boolean_and_candidate:?}"
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
                .pinned(pin())
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

    let structural_typed_expr = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .structural()
                .query()
                .native("match { function_item { { :[name.expr] } } }")
                .pinned(pin())
                .top_k(2)
                .execute()
        },
        |response| response.generation == pin() && response.results.len() == 1,
    )?;
    let structural_typed_expr_candidate = structural_typed_expr
        .results
        .first()
        .ok_or_else(|| "missing structural typed-expr candidate".to_string())?;
    let structural_typed_expr_binding = structural_typed_expr_candidate
        .bindings
        .first()
        .ok_or_else(|| "missing structural typed-expr binding".to_string())?;
    if structural_typed_expr_candidate.candidate_id != "chunk-tree"
        || structural_typed_expr_binding.metavariable != "name"
        || structural_typed_expr_binding.start_byte != 3
        || structural_typed_expr_binding.end_byte != 7
    {
        stop_runtime(&shutdown, join)?;
        return Err(format!(
            "unexpected structural typed-expr candidate/binding: \
             {structural_typed_expr_candidate:?}"
        )
        .into());
    }

    let structural_typed_item = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .structural()
                .query()
                .native("match { :[root.item] }")
                .pinned(pin())
                .top_k(2)
                .execute()
        },
        |response| response.generation == pin() && response.results.len() == 1,
    )?;
    let structural_typed_item_candidate = structural_typed_item
        .results
        .first()
        .ok_or_else(|| "missing structural typed-item candidate".to_string())?;
    let structural_typed_item_binding = structural_typed_item_candidate
        .bindings
        .first()
        .ok_or_else(|| "missing structural typed-item binding".to_string())?;
    if structural_typed_item_candidate.candidate_id != "chunk-tree"
        || structural_typed_item_binding.metavariable != "root"
        || structural_typed_item_binding.start_byte != 0
        || structural_typed_item_binding.end_byte != 10
    {
        stop_runtime(&shutdown, join)?;
        return Err(format!(
            "unexpected structural typed-item candidate/binding: \
             {structural_typed_item_candidate:?}"
        )
        .into());
    }

    let structural_typed_stmt = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .structural()
                .query()
                .native("match { function_item { { :[body.stmt] } } }")
                .pinned(pin())
                .top_k(2)
                .execute()
        },
        |response| response.generation == pin() && response.results.len() == 1,
    )?;
    let structural_typed_stmt_candidate = structural_typed_stmt
        .results
        .first()
        .ok_or_else(|| "missing structural typed-stmt candidate".to_string())?;
    let structural_typed_stmt_binding = structural_typed_stmt_candidate
        .bindings
        .first()
        .ok_or_else(|| "missing structural typed-stmt binding".to_string())?;
    if structural_typed_stmt_candidate.candidate_id != "chunk-tree"
        || structural_typed_stmt_binding.metavariable != "body"
        || structural_typed_stmt_binding.start_byte != 8
        || structural_typed_stmt_binding.end_byte != 10
    {
        stop_runtime(&shutdown, join)?;
        return Err(format!(
            "unexpected structural typed-stmt candidate/binding: \
             {structural_typed_stmt_candidate:?}"
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
                .pinned(pin())
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
                .pinned(pin())
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
                .pinned(pin())
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
        .pinned(pin())
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
                .pinned(pin())
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
                .pinned(pin())
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
                .pinned(pin())
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

    let structural_sourcegraph = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .structural()
                .query()
                .sourcegraph(
                    r#"repo:repo-sdk path:src/lib.rs lang:rust patterntype:structural "function_item { { identifier :[name] } }""#,
                )
                .pinned(pin())
                .top_k(2)
                .execute()
        },
        |response| response.generation == pin() && response.results.len() == 1,
    )?;
    if structural_sourcegraph.generation != pin() || structural_sourcegraph.results.len() != 1 {
        stop_runtime(&shutdown, join)?;
        return Err(format!(
            "unexpected Sourcegraph structural response: {structural_sourcegraph:?}"
        )
        .into());
    }
    let structural_sourcegraph_candidate = structural_sourcegraph
        .results
        .first()
        .ok_or_else(|| "missing Sourcegraph structural candidate".to_string())?;
    let structural_sourcegraph_binding = structural_sourcegraph_candidate
        .bindings
        .first()
        .ok_or_else(|| "missing Sourcegraph structural binding".to_string())?;
    if structural_sourcegraph_candidate.candidate_id != "chunk-tree"
        || structural_sourcegraph_binding.metavariable != "name"
        || structural_sourcegraph_binding.start_byte != 3
        || structural_sourcegraph_binding.end_byte != 7
    {
        stop_runtime(&shutdown, join)?;
        return Err(format!(
            "unexpected Sourcegraph structural candidate/binding: \
             {structural_sourcegraph_candidate:?}"
        )
        .into());
    }

    let structural_sourcegraph_regex = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .structural()
                .query()
                .sourcegraph(
                    r"repo:repo-sdk path:src/lib.rs lang:rust patterntype:structural /^main$/",
                )
                .pinned(pin())
                .top_k(2)
                .execute()
        },
        |response| response.generation == pin() && response.results.len() == 1,
    )?;
    if structural_sourcegraph_regex.generation != pin()
        || structural_sourcegraph_regex.results.len() != 1
    {
        stop_runtime(&shutdown, join)?;
        return Err(format!(
            "unexpected Sourcegraph structural regex response: {structural_sourcegraph_regex:?}"
        )
        .into());
    }
    let structural_sourcegraph_regex_candidate = structural_sourcegraph_regex
        .results
        .first()
        .ok_or_else(|| "missing Sourcegraph structural regex candidate".to_string())?;
    let structural_sourcegraph_regex_binding = structural_sourcegraph_regex_candidate
        .bindings
        .first()
        .ok_or_else(|| "missing Sourcegraph structural regex binding".to_string())?;
    if structural_sourcegraph_regex_candidate.candidate_id != "chunk-tree"
        || !structural_sourcegraph_regex_binding
            .metavariable
            .starts_with("__sg_regex_")
        || structural_sourcegraph_regex_binding.start_byte != 3
        || structural_sourcegraph_regex_binding.end_byte != 7
    {
        stop_runtime(&shutdown, join)?;
        return Err(format!(
            "unexpected Sourcegraph structural regex candidate/binding: \
             {structural_sourcegraph_regex_candidate:?}"
        )
        .into());
    }

    let structural_repo_miss = client
        .structural()
        .query()
        .native("repo:other-repo match { :[x] }")
        .pinned(pin())
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
        .pinned(pin())
        .top_k(2)
        .execute()
    else {
        stop_runtime(&shutdown, join)?;
        return Err("invalid-filter structural query unexpectedly succeeded".into());
    };
    expect_remote_code(structural_invalid_request_err, "STR_INVALID_REQUEST")?;

    let Err(structural_sourcegraph_invalid_request_err) = client
        .structural()
        .query()
        .sourcegraph(r#"select:repo patterntype:structural "function_item""#)
        .pinned(pin())
        .top_k(2)
        .execute()
    else {
        stop_runtime(&shutdown, join)?;
        return Err("invalid Sourcegraph structural filter unexpectedly succeeded".into());
    };
    expect_remote_code(
        structural_sourcegraph_invalid_request_err,
        "STR_INVALID_REQUEST",
    )?;

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

#[test]
fn sdk_frontdoor_widened_query_matrix_executes_exact_surface_truth() -> TestResult {
    let (_dir, client, ingest_socket, shutdown, join) =
        start_sdk_frontdoor_runtime_with_ingest("sdk-frontdoor-widened-query-matrix")?;

    let _lexical_receipt = client
        .lexical()
        .publish(&lexical_frontdoor_matrix_batch()?)?;
    let _history_receipt = client.history().publish(&history_batch())?;
    let _dirty_receipt = client.runtime().publish_dirty(&dirty_batch())?;
    publish_runtime_catalog_batch(&ingest_socket)?;
    let _activation = wait_for_sdk_ready(SOCKET_TIMEOUT, || {
        client
            .generations()
            .activate()
            .repo(repo())
            .revision(revision())
            .generation(generation())
            .manifest_digest("manifest:sdk-frontdoor-matrix")
            .tracks([SearchPlaneTrackKind::Lexical])?
            .commit()
    })?;

    for &scenario in SDK_FRONTDOOR_SCENARIOS {
        match scenario.expected {
            SdkFrontdoorExpectation::CandidateIds(expected_ids) => match scenario.surface {
                SdkFrontdoorSurface::Lexical => {
                    let response = wait_for_sdk_observation(
                        SOCKET_TIMEOUT,
                        || match scenario.syntax {
                            TextQuerySyntax::Native => client
                                .lexical()
                                .query()
                                .native(scenario.query_text)
                                .active(repo(), revision())
                                .top_k(10)
                                .execute(),
                            TextQuerySyntax::Sourcegraph => client
                                .lexical()
                                .query()
                                .sourcegraph(scenario.query_text)
                                .active(repo(), revision())
                                .top_k(10)
                                .execute(),
                        },
                        |response| response.generation == pin(),
                    )?;
                    let observed = response
                        .results
                        .iter()
                        .map(|candidate| candidate.candidate_id.clone())
                        .collect::<Vec<_>>();
                    let expected = expected_ids
                        .iter()
                        .map(|id| (*id).to_string())
                        .collect::<Vec<_>>();
                    if observed != expected {
                        stop_runtime(&shutdown, join)?;
                        return Err(format!(
                            "{} lexical candidate drift: expected {:?}, got {:?}",
                            scenario.name, expected, observed
                        )
                        .into());
                    }
                }
                SdkFrontdoorSurface::Symbol => {
                    let response =
                        wait_for_symbol_query(SOCKET_TIMEOUT, || match scenario.syntax {
                            TextQuerySyntax::Native => client
                                .symbol()
                                .query()
                                .native(scenario.query_text)
                                .active(repo(), revision())
                                .top_k(10)
                                .execute(),
                            TextQuerySyntax::Sourcegraph => client
                                .symbol()
                                .query()
                                .sourcegraph(scenario.query_text)
                                .active(repo(), revision())
                                .top_k(10)
                                .execute(),
                        })?;
                    let observed = response
                        .results
                        .iter()
                        .map(|candidate| candidate.candidate_id.clone())
                        .collect::<Vec<_>>();
                    let expected = expected_ids
                        .iter()
                        .map(|id| (*id).to_string())
                        .collect::<Vec<_>>();
                    if observed != expected {
                        stop_runtime(&shutdown, join)?;
                        return Err(format!(
                            "{} symbol candidate drift: expected {:?}, got {:?}",
                            scenario.name, expected, observed
                        )
                        .into());
                    }
                }
                SdkFrontdoorSurface::RuntimeMetadata => {
                    let response = wait_for_sdk_observation(
                        SOCKET_TIMEOUT,
                        || match scenario.syntax {
                            TextQuerySyntax::Native => client
                                .runtime()
                                .query()
                                .native(scenario.query_text)
                                .active(repo(), revision())
                                .top_k(10)
                                .execute(),
                            TextQuerySyntax::Sourcegraph => client
                                .runtime()
                                .query()
                                .sourcegraph(scenario.query_text)
                                .active(repo(), revision())
                                .top_k(10)
                                .execute(),
                        },
                        |response| response.generation == pin(),
                    )?;
                    let observed = response
                        .results
                        .iter()
                        .map(|candidate| candidate.candidate_id.clone())
                        .collect::<Vec<_>>();
                    let expected = expected_ids
                        .iter()
                        .map(|id| (*id).to_string())
                        .collect::<Vec<_>>();
                    if observed != expected {
                        stop_runtime(&shutdown, join)?;
                        return Err(format!(
                            "{} runtime candidate drift: expected {:?}, got {:?}",
                            scenario.name, expected, observed
                        )
                        .into());
                    }
                }
                SdkFrontdoorSurface::History => {
                    stop_runtime(&shutdown, join)?;
                    return Err(format!(
                        "{} used candidate-id expectation on history surface",
                        scenario.name
                    )
                    .into());
                }
            },
            SdkFrontdoorExpectation::CommitShas(expected_shas) => {
                if scenario.surface != SdkFrontdoorSurface::History {
                    stop_runtime(&shutdown, join)?;
                    return Err(format!(
                        "{} used commit expectation on non-history surface",
                        scenario.name
                    )
                    .into());
                }
                let response = wait_for_sdk_observation(
                    SOCKET_TIMEOUT,
                    || match scenario.syntax {
                        TextQuerySyntax::Native => client
                            .history()
                            .query()
                            .native(scenario.query_text)
                            .active(repo(), revision())
                            .top_k(10)
                            .execute(),
                        TextQuerySyntax::Sourcegraph => client
                            .history()
                            .query()
                            .sourcegraph(scenario.query_text)
                            .active(repo(), revision())
                            .top_k(10)
                            .execute(),
                    },
                    |response| response.generation == pin(),
                )?;
                let observed = response
                    .commits
                    .iter()
                    .map(|commit| commit.sha.to_hex())
                    .collect::<Vec<_>>();
                let expected = expected_shas
                    .iter()
                    .map(|sha| (*sha).to_string())
                    .collect::<Vec<_>>();
                if observed != expected || !response.diffs.is_empty() {
                    stop_runtime(&shutdown, join)?;
                    return Err(format!(
                        "{} history commit drift: expected {:?}, got commits={:?} diffs={:?}",
                        scenario.name, expected, observed, response.diffs
                    )
                    .into());
                }
            }
            SdkFrontdoorExpectation::TypedError(expected_error) => {
                if scenario.surface != SdkFrontdoorSurface::History {
                    stop_runtime(&shutdown, join)?;
                    return Err(format!(
                        "{} typed error expectation on unsupported SDK surface",
                        scenario.name
                    )
                    .into());
                }
                let err =
                    wait_for_sdk_terminal_error(SOCKET_TIMEOUT, &["NOT_READY"], || match scenario
                        .syntax
                    {
                        TextQuerySyntax::Native => client
                            .history()
                            .query()
                            .native(scenario.query_text)
                            .active(repo(), revision())
                            .top_k(10)
                            .execute(),
                        TextQuerySyntax::Sourcegraph => client
                            .history()
                            .query()
                            .sourcegraph(scenario.query_text)
                            .active(repo(), revision())
                            .top_k(10)
                            .execute(),
                    })?;
                match err {
                    SdkError::Remote { code, message }
                        if code == expected_error.code
                            && message.contains(expected_error.message_contains) => {}
                    other => {
                        stop_runtime(&shutdown, join)?;
                        return Err(format!(
                            "{} typed error drifted: expected code={} fragment={:?}, got {other:?}",
                            scenario.name, expected_error.code, expected_error.message_contains
                        )
                        .into());
                    }
                }
            }
        }
    }

    stop_runtime(&shutdown, join)
}

#[test]
fn sdk_history_query_frontdoor_surfaces_typed_absent_and_shard_errors() -> TestResult {
    let dir = tempfile::tempdir()?;
    let runtime = build_runtime(build_config(dir.path()))?;
    let query_socket = runtime.query_server.socket_path().to_path_buf();
    let control_socket = runtime.control_server.socket_path().to_path_buf();
    let ingest_socket = runtime.ingest_server.socket_path().to_path_buf();
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_for_drive = Arc::clone(&shutdown);
    let join = thread::Builder::new()
        .name("sdk-frontdoor-history-errors".into())
        .spawn(move || drive(runtime, &shutdown_for_drive))?;

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

    let generation_not_ready = expect_sdk_error(
        client
            .history()
            .query()
            .sourcegraph("type:commit fix")
            .pinned(pin())
            .top_k(5)
            .execute(),
        "history query without materialized authority should fail",
    )?;
    expect_remote_code(generation_not_ready, "HISTORY_GENERATION_NOT_READY")?;

    let _lexical_receipt = client.lexical().publish(&lexical_batch()?)?;
    let producer_unavailable = expect_sdk_error(
        client
            .history()
            .query()
            .sourcegraph("type:commit todo")
            .pinned(pin())
            .top_k(5)
            .execute(),
        "history query should not fall back to lexical content",
    )?;
    expect_remote_code(producer_unavailable, "HISTORY_PRODUCER_UNAVAILABLE")?;

    let _history_receipt = client.history().publish(&history_commit_only_batch())?;
    let shard_unavailable = expect_sdk_error(
        client
            .history()
            .query()
            .sourcegraph("type:diff todo")
            .pinned(pin())
            .top_k(5)
            .execute(),
        "history diff query should fail when diff shard is absent",
    )?;
    expect_remote_code(shard_unavailable, "HISTORY_SHARD_UNAVAILABLE")?;

    stop_runtime(&shutdown, join)
}

#[test]
fn sdk_structural_sourcegraph_frontdoor_supports_boolean_and_typed_hole_truth() -> TestResult {
    let (_dir, client, shutdown, join) =
        start_sdk_frontdoor_runtime("sdk-frontdoor-structural-sourcegraph-v2")?;

    publish_sdk_lexical_and_structural_ready(&client)?;

    let typed_expr = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .structural()
                .query()
                .sourcegraph(r#"patterntype:structural "function_item { { :[name.expr] } }""#)
                .pinned(pin())
                .top_k(2)
                .execute()
        },
        |response| response.generation == pin() && response.results.len() == 1,
    )?;
    assert_structural_single_binding(
        &typed_expr,
        &pin(),
        "chunk-tree",
        "name",
        3,
        7,
        "sdk structural Sourcegraph typed expr",
    )?;

    let boolean_or = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .structural()
                .query()
                .sourcegraph(
                    r#"patterntype:structural "function_item { { identifier :[name] } }" OR "trait_item""#,
                )
                .pinned(pin())
                .top_k(2)
                .execute()
        },
        |response| response.generation == pin() && response.results.len() == 1,
    )?;
    assert_structural_single_binding(
        &boolean_or,
        &pin(),
        "chunk-tree",
        "name",
        3,
        7,
        "sdk structural Sourcegraph boolean OR",
    )?;

    let boolean_not = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .structural()
                .query()
                .sourcegraph(
                    r#"patterntype:structural "function_item { { identifier :[name] } }" AND NOT "trait_item""#,
                )
                .pinned(pin())
                .top_k(2)
                .execute()
        },
        |response| response.generation == pin() && response.results.len() == 1,
    )?;
    assert_structural_single_binding(
        &boolean_not,
        &pin(),
        "chunk-tree",
        "name",
        3,
        7,
        "sdk structural Sourcegraph boolean NOT",
    )?;

    stop_runtime(&shutdown, join)
}

#[test]
fn sdk_dsl_frontdoor_fail_closed_timeout_and_recovery_truth() -> TestResult {
    let (_dir, client, shutdown, join) =
        start_sdk_frontdoor_runtime("sdk-frontdoor-dsl-fail-closed")?;

    publish_sdk_lexical_and_structural_ready(&client)?;

    let lexical_timeout = expect_sdk_error(
        client
            .lexical()
            .query()
            .sourcegraph(r"timeout:0ms /todo!/")
            .active(repo(), revision())
            .top_k(2)
            .execute(),
        "Sourcegraph lexical timeout must fail closed",
    )?;
    match lexical_timeout {
        SdkError::Remote { code, message }
            if code == "QUERY_TIMEOUT"
                && (message.contains("timeout") || message.contains("timed out")) => {}
        other @ (SdkError::Usage(_)
        | SdkError::Protocol(_)
        | SdkError::Serialization(_)
        | SdkError::Transport(_)
        | SdkError::Remote { .. }) => {
            stop_runtime(&shutdown, join)?;
            return Err(format!("unexpected lexical timeout error: {other:?}").into());
        }
    }

    let lexical_follow_up = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .lexical()
                .query()
                .sourcegraph("todo")
                .active(repo(), revision())
                .top_k(2)
                .execute()
        },
        |response| response.generation == pin() && response.results.len() == 1,
    )?;
    let lexical_follow_up_candidate = lexical_follow_up
        .results
        .first()
        .ok_or_else(|| "missing lexical follow-up candidate".to_string())?;
    if lexical_follow_up.generation != pin()
        || lexical_follow_up_candidate.candidate_id != "chunk-dirty"
    {
        stop_runtime(&shutdown, join)?;
        return Err(
            format!("unexpected lexical follow-up after timeout: {lexical_follow_up:?}").into(),
        );
    }

    let structural_timeout = expect_sdk_error(
        client
            .structural()
            .query()
            .sourcegraph(r#"timeout:0ms patterntype:structural "function_item""#)
            .pinned(pin())
            .top_k(2)
            .execute(),
        "Sourcegraph structural timeout must fail closed",
    )?;
    match structural_timeout {
        SdkError::Remote { code, message }
            if code == "STR_INVALID_REQUEST" && message.contains("timeout option") => {}
        other @ (SdkError::Usage(_)
        | SdkError::Protocol(_)
        | SdkError::Serialization(_)
        | SdkError::Transport(_)
        | SdkError::Remote { .. }) => {
            stop_runtime(&shutdown, join)?;
            return Err(format!("unexpected structural timeout error: {other:?}").into());
        }
    }

    let typed_hole = expect_sdk_error(
        client
            .structural()
            .query()
            .native("match { function_item { { :[name.lambda] } } }")
            .pinned(pin())
            .top_k(2)
            .execute(),
        "unsupported typed hole must fail closed",
    )?;
    match typed_hole {
        SdkError::Remote { code, message }
            if code == "STR_HOLE_KIND_UNSUPPORTED"
                && message.contains("typed hole kind `lambda`") => {}
        other @ (SdkError::Usage(_)
        | SdkError::Protocol(_)
        | SdkError::Serialization(_)
        | SdkError::Transport(_)
        | SdkError::Remote { .. }) => {
            stop_runtime(&shutdown, join)?;
            return Err(format!("unexpected typed-hole error: {other:?}").into());
        }
    }

    let mixed_boolean = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .structural()
                .query()
                .native("main AND match { function_item :[x] }")
                .pinned(pin())
                .top_k(2)
                .execute()
        },
        |response| response.generation == pin() && response.results.len() == 1,
    )?;
    assert_structural_single_binding(
        &mixed_boolean,
        &pin(),
        "chunk-tree",
        "x",
        0,
        10,
        "sdk mixed lexical/structural boolean AND",
    )?;

    let mixed_or = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .structural()
                .query()
                .native("main OR match { function_item :[x] }")
                .pinned(pin())
                .top_k(10)
                .execute()
        },
        |response| {
            response.generation == pin()
                && response.results.len() == 1
                && response
                    .results
                    .first()
                    .map(|candidate| !candidate.bindings.is_empty())
                    .unwrap_or(false)
        },
    )?;
    if mixed_or.results.len() != 1 {
        stop_runtime(&shutdown, join)?;
        return Err(format!(
            "sdk mixed lexical/structural OR expected one surviving candidate, got {:?}",
            mixed_or.results
        )
        .into());
    }
    let mixed_or_candidate = mixed_or
        .results
        .first()
        .ok_or_else(|| "sdk mixed lexical/structural boolean OR: missing candidate".to_string())?;
    if mixed_or.generation != pin() || mixed_or_candidate.candidate_id != "chunk-tree" {
        stop_runtime(&shutdown, join)?;
        return Err(format!(
            "sdk mixed lexical/structural boolean OR: unexpected response {mixed_or:?}"
        )
        .into());
    }

    let mixed_and_not = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .structural()
                .query()
                .native("main AND NOT match { trait_item }")
                .pinned(pin())
                .top_k(10)
                .execute()
        },
        |response| response.generation == pin() && response.results.len() == 1,
    )?;
    let mixed_and_not_candidate = mixed_and_not.results.first().ok_or_else(|| {
        "sdk mixed lexical/structural boolean AND NOT: missing candidate".to_string()
    })?;
    if mixed_and_not.generation != pin() || mixed_and_not_candidate.candidate_id != "chunk-tree" {
        stop_runtime(&shutdown, join)?;
        return Err(format!(
            "sdk mixed lexical/structural boolean AND NOT: unexpected response {mixed_and_not:?}"
        )
        .into());
    }

    let pure_negative = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .structural()
                .query()
                .native("NOT match { function_item }")
                .pinned(pin())
                .top_k(10)
                .execute()
        },
        |response| {
            response.generation == pin()
                && !response.results.is_empty()
                && response
                    .results
                    .iter()
                    .all(|candidate| candidate.candidate_id != "chunk-tree")
        },
    )?;
    for candidate in &pure_negative.results {
        if candidate.candidate_id == "chunk-tree" {
            stop_runtime(&shutdown, join)?;
            return Err(format!(
                "pure-negative root must exclude function_item matches, got {candidate:?}"
            )
            .into());
        }
        if !candidate.bindings.is_empty() {
            stop_runtime(&shutdown, join)?;
            return Err(format!(
                "pure-negative universe placeholder must not invent bindings, got {candidate:?}"
            )
            .into());
        }
    }

    let missing_patterntype = expect_sdk_error(
        client
            .structural()
            .query()
            .sourcegraph(r#""function_item""#)
            .pinned(pin())
            .top_k(2)
            .execute(),
        "Sourcegraph structural route requires patterntype:structural",
    )?;
    match missing_patterntype {
        SdkError::Remote { code, message }
            if code == "BRIDGE_TRANSLATE_FAIL" && message.contains("patterntype:structural") => {}
        other @ (SdkError::Usage(_)
        | SdkError::Protocol(_)
        | SdkError::Serialization(_)
        | SdkError::Transport(_)
        | SdkError::Remote { .. }) => {
            stop_runtime(&shutdown, join)?;
            return Err(format!("unexpected patterntype error: {other:?}").into());
        }
    }

    let structural_follow_up = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .structural()
                .query()
                .native("match { function_item { { :[name.expr] } } }")
                .pinned(pin())
                .top_k(2)
                .execute()
        },
        |response| response.generation == pin() && response.results.len() == 1,
    )?;
    assert_structural_single_binding(
        &structural_follow_up,
        &pin(),
        "chunk-tree",
        "name",
        3,
        7,
        "sdk structural follow-up after typed errors",
    )?;

    stop_runtime(&shutdown, join)
}

#[test]
fn sdk_contract_exact_query_request_frontdoors_roundtrip_truth() -> TestResult {
    let (_dir, client, shutdown, join) =
        start_sdk_frontdoor_runtime("sdk-frontdoor-contract-exact")?;

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
            .tracks([
                SearchPlaneTrackKind::Lexical,
                SearchPlaneTrackKind::Semantic,
                SearchPlaneTrackKind::Structural,
            ])?
            .commit()
    })?;

    let lexical_request = TextQueryRequest {
        syntax: TextQuerySyntax::Native,
        query_text: "todo".to_string(),
        generation: None,
        generation_selector: Some(active_selector()),
        top_k: 2,
    };
    let lexical = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || client.lexical().query_request(lexical_request.clone()),
        |response| response.generation == pin() && response.results.len() == 1,
    )?;
    let lexical_candidate = lexical
        .results
        .first()
        .ok_or_else(|| "missing contract-exact lexical candidate".to_string())?;
    if lexical.generation != pin()
        || lexical_candidate.candidate_id != "chunk-dirty"
        || lexical_candidate.repo_relative_path.as_str() != "src/lib.rs"
    {
        stop_runtime(&shutdown, join)?;
        return Err(format!("unexpected contract-exact lexical response: {lexical:?}").into());
    }

    let symbol_request = SymbolQueryRequest {
        syntax: TextQuerySyntax::Sourcegraph,
        query_text: "select:symbol MySdkSymbol".to_string(),
        generation: None,
        generation_selector: Some(active_selector()),
        top_k: 3,
    };
    let symbol = wait_for_symbol_query(SOCKET_TIMEOUT, || {
        client.symbol().query_request(symbol_request.clone())
    })?;
    assert_single_symbol_candidate(&symbol)?;

    let history_request = HistoryQueryRequest {
        text_query: TextQueryRequest {
            syntax: TextQuerySyntax::Sourcegraph,
            query_text: "type:commit rev:refs/heads/main author:alice fix".to_string(),
            generation: None,
            generation_selector: Some(active_selector()),
            top_k: 5,
        },
    };
    let history = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || client.history().query_request(history_request.clone()),
        |response| response.generation == pin() && response.commits.len() == 1,
    )?;
    let history_commit = history
        .commits
        .first()
        .ok_or_else(|| "missing contract-exact history candidate".to_string())?;
    if history.generation != pin()
        || history_commit.author != "alice"
        || history_commit.message != "fix: sample"
    {
        stop_runtime(&shutdown, join)?;
        return Err(format!("unexpected contract-exact history response: {history:?}").into());
    }

    let runtime_request = RuntimeMetadataQueryRequest {
        text_query: TextQueryRequest {
            syntax: TextQuerySyntax::Sourcegraph,
            query_text: "dirty:yes todo".to_string(),
            generation: None,
            generation_selector: Some(active_selector()),
            top_k: 3,
        },
    };
    let runtime = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || client.runtime().query_request(runtime_request.clone()),
        |response| response.generation == pin() && response.results.len() == 1,
    )?;
    let runtime_top = runtime
        .results
        .first()
        .ok_or_else(|| "missing contract-exact runtime candidate".to_string())?;
    if runtime.generation != pin() || runtime_top.candidate_id != "chunk-dirty" {
        stop_runtime(&shutdown, join)?;
        return Err(format!("unexpected contract-exact runtime response: {runtime:?}").into());
    }

    let structural_request = StructuralQueryRequest {
        text_query: TextQueryRequest {
            syntax: TextQuerySyntax::Sourcegraph,
            query_text:
                r#"repo:repo-sdk path:src/lib.rs lang:rust patterntype:structural "function_item { { :[name.expr] } }""#
                    .to_string(),
            generation: None,
            generation_selector: Some(active_selector()),
            top_k: 2,
        },
    };
    let structural = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .structural()
                .query_request(structural_request.clone())
        },
        |response| response.generation == pin() && response.results.len() == 1,
    )?;
    let structural_top = structural
        .results
        .first()
        .ok_or_else(|| "missing contract-exact structural candidate".to_string())?;
    let structural_binding = structural_top
        .bindings
        .first()
        .ok_or_else(|| "missing contract-exact structural binding".to_string())?;
    if structural.generation != pin()
        || structural_top.candidate_id != "chunk-tree"
        || structural_binding.metavariable != "name"
        || structural_binding.start_byte != 3
        || structural_binding.end_byte != 7
    {
        stop_runtime(&shutdown, join)?;
        return Err(
            format!("unexpected contract-exact structural response: {structural:?}").into(),
        );
    }

    let semantic_request = SemanticQueryRequest {
        query_text: "quartz".to_string(),
        generation: None,
        generation_selector: Some(active_selector()),
        lexical_scope: Some(TextQueryRequest {
            syntax: TextQuerySyntax::Sourcegraph,
            query_text: "sphinx".to_string(),
            generation: None,
            generation_selector: Some(active_selector()),
            top_k: 2,
        }),
        top_k: 2,
    };
    let semantic = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || client.semantic().query_request(semantic_request.clone()),
        |response| response.generation == pin() && !response.results.is_empty(),
    )?;
    let semantic_top = semantic
        .results
        .first()
        .ok_or_else(|| "missing contract-exact semantic candidate".to_string())?;
    if semantic.generation != pin()
        || semantic_top.candidate_id != "alpha"
        || semantic.explanation.summary.is_empty()
    {
        stop_runtime(&shutdown, join)?;
        return Err(format!("unexpected contract-exact semantic response: {semantic:?}").into());
    }

    let hybrid_request = HybridQueryRequest {
        text_query: TextQueryRequest {
            syntax: TextQuerySyntax::Native,
            query_text: "sphinx".to_string(),
            generation: None,
            generation_selector: Some(active_selector()),
            top_k: 2,
        },
        semantic_query_text: "quartz".to_string(),
        generation: None,
        generation_selector: Some(active_selector()),
        top_k: 2,
    };
    let hybrid = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || client.search().hybrid_request(hybrid_request.clone()),
        |response| response.generation == pin() && !response.results.is_empty(),
    )?;
    let hybrid_top = hybrid
        .results
        .first()
        .ok_or_else(|| "missing contract-exact hybrid candidate".to_string())?;
    if hybrid.generation != pin()
        || hybrid_top.candidate_id != "alpha"
        || hybrid.explanation.summary.is_empty()
    {
        stop_runtime(&shutdown, join)?;
        return Err(format!("unexpected contract-exact hybrid response: {hybrid:?}").into());
    }

    stop_runtime(&shutdown, join)
}

#[test]
fn sdk_generations_frontdoor_routes_commit_current_status_and_builder_activation() -> TestResult {
    let (_dir, client, shutdown, join) = start_sdk_frontdoor_runtime("sdk-frontdoor-generations")?;

    let initial_status = client.generations().status(repo(), revision())?;
    if initial_status.repo_id != repo()
        || initial_status.revision_id != revision()
        || !initial_status.tracks.is_empty()
    {
        stop_runtime(&shutdown, join)?;
        return Err(format!("unexpected initial generation status: {initial_status:?}").into());
    }

    let not_ready = expect_sdk_error(
        client
            .generations()
            .current(repo(), revision(), SearchPlaneTrackKind::Lexical),
        "generation current before activation should fail closed",
    )?;
    expect_remote_code(not_ready, "NOT_READY")?;

    let direct_activation_not_ready = expect_sdk_error(
        client
            .generations()
            .commit(SearchPlaneActivateGenerationRequest {
                repo_id: repo(),
                revision_id: revision(),
                manifest_generation: generation(),
                manifest_digest: "manifest:direct-activation".to_string(),
                tracks: vec![SearchPlaneTrackKind::Lexical],
            }),
        "direct activation before lexical materialization should fail closed",
    )?;
    expect_remote_code(direct_activation_not_ready, "NOT_READY")?;

    let _lexical_receipt = client.lexical().publish(&lexical_batch()?)?;
    let _structural_receipt = client.structural().publish(&structural_batch()?)?;

    let direct_activation = client
        .generations()
        .commit(SearchPlaneActivateGenerationRequest {
            repo_id: repo(),
            revision_id: revision(),
            manifest_generation: generation(),
            manifest_digest: "manifest:direct-activation".to_string(),
            tracks: vec![SearchPlaneTrackKind::Lexical],
        })?;
    if direct_activation.repo_id != repo()
        || direct_activation.revision_id != revision()
        || direct_activation.manifest_generation != generation()
        || direct_activation.manifest_digest != "manifest:direct-activation"
        || direct_activation.tracks != vec![SearchPlaneTrackKind::Lexical]
    {
        stop_runtime(&shutdown, join)?;
        return Err(format!("unexpected direct activation ack: {direct_activation:?}").into());
    }

    let lexical_snapshot = wait_for_sdk_ready(SOCKET_TIMEOUT, || {
        client
            .generations()
            .current(repo(), revision(), SearchPlaneTrackKind::Lexical)
    })?;
    if lexical_snapshot.repo_id != repo()
        || lexical_snapshot.revision_id != revision()
        || lexical_snapshot.track != SearchPlaneTrackKind::Lexical
        || lexical_snapshot.manifest_generation != generation()
        || lexical_snapshot.manifest_digest != "manifest:direct-activation"
    {
        stop_runtime(&shutdown, join)?;
        return Err(format!("unexpected lexical generation snapshot: {lexical_snapshot:?}").into());
    }

    let builder_activation = client
        .generations()
        .activate()
        .repo(repo())
        .revision(revision())
        .generation(generation())
        .manifest_digest("manifest:builder-activation")
        .track(SearchPlaneTrackKind::Structural)
        .commit()?;
    if builder_activation.tracks != vec![SearchPlaneTrackKind::Structural]
        || builder_activation.manifest_digest != "manifest:builder-activation"
    {
        stop_runtime(&shutdown, join)?;
        return Err(format!("unexpected builder activation ack: {builder_activation:?}").into());
    }

    let structural_snapshot = wait_for_sdk_ready(SOCKET_TIMEOUT, || {
        client
            .generations()
            .current(repo(), revision(), SearchPlaneTrackKind::Structural)
    })?;
    if structural_snapshot.repo_id != repo()
        || structural_snapshot.revision_id != revision()
        || structural_snapshot.track != SearchPlaneTrackKind::Structural
        || structural_snapshot.manifest_generation != generation()
        || structural_snapshot.manifest_digest != "manifest:builder-activation"
    {
        stop_runtime(&shutdown, join)?;
        return Err(
            format!("unexpected structural generation snapshot: {structural_snapshot:?}").into(),
        );
    }

    let final_status = wait_for_sdk_ready(SOCKET_TIMEOUT, || {
        client.generations().status(repo(), revision())
    })?;
    if final_status.repo_id != repo() || final_status.revision_id != revision() {
        stop_runtime(&shutdown, join)?;
        return Err(format!("unexpected final generation status: {final_status:?}").into());
    }
    match final_status.tracks.as_slice() {
        [lexical, structural]
            if lexical.track == SearchPlaneTrackKind::Lexical
                && lexical.manifest_digest == "manifest:direct-activation"
                && structural.track == SearchPlaneTrackKind::Structural
                && structural.manifest_digest == "manifest:builder-activation" => {}
        _ => {
            stop_runtime(&shutdown, join)?;
            return Err(format!("unexpected final generation track set: {final_status:?}").into());
        }
    }

    stop_runtime(&shutdown, join)
}

#[test]
fn sdk_builder_variant_frontdoors_route_native_inline_vector_and_pinned_truth() -> TestResult {
    let (_dir, client, shutdown, join) =
        start_sdk_frontdoor_runtime("sdk-frontdoor-builder-variants")?;

    let _lexical_receipt = client.lexical().publish(&lexical_batch()?)?;
    let _dirty_receipt = client.runtime().publish_dirty(&dirty_batch())?;
    let _activation = wait_for_sdk_ready(SOCKET_TIMEOUT, || {
        client
            .generations()
            .activate()
            .repo(repo())
            .revision(revision())
            .generation(generation())
            .manifest_digest("manifest:lexical")
            .tracks([
                SearchPlaneTrackKind::Lexical,
                SearchPlaneTrackKind::Semantic,
            ])?
            .commit()
    })?;

    let lexical_native = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .lexical()
                .query()
                .native("todo")
                .pinned(pin())
                .top_k(2)
                .execute()
        },
        |response| response.generation == pin() && response.results.len() == 1,
    )?;
    let lexical_native_candidate = lexical_native
        .results
        .first()
        .ok_or_else(|| "missing lexical native candidate".to_string())?;
    if lexical_native.generation != pin()
        || lexical_native_candidate.candidate_id != "chunk-dirty"
        || lexical_native_candidate.repo_relative_path.as_str() != "src/lib.rs"
    {
        stop_runtime(&shutdown, join)?;
        return Err(format!("unexpected lexical native response: {lexical_native:?}").into());
    }

    let runtime_native = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .runtime()
                .query()
                .native("dirty:yes todo")
                .pinned(pin())
                .top_k(3)
                .execute()
        },
        |response| response.generation == pin() && response.results.len() == 1,
    )?;
    let runtime_native_candidate = runtime_native
        .results
        .first()
        .ok_or_else(|| "missing runtime native candidate".to_string())?;
    if runtime_native.generation != pin()
        || runtime_native_candidate.candidate_id != "chunk-dirty"
        || runtime_native_candidate.repo_relative_path.as_str() != "src/lib.rs"
    {
        stop_runtime(&shutdown, join)?;
        return Err(format!("unexpected runtime native response: {runtime_native:?}").into());
    }

    let semantic_inline = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .semantic()
                .query()
                .text("quartz")
                .scope_native("sphinx")
                .scope_top_k(2)
                .pinned(pin())
                .top_k(2)
                .execute()
        },
        |response| response.generation == pin() && !response.results.is_empty(),
    )?;
    let semantic_inline_top = semantic_inline
        .results
        .first()
        .ok_or_else(|| "missing semantic inline-vector candidate".to_string())?;
    if semantic_inline.generation != pin()
        || semantic_inline_top.candidate_id != "alpha"
        || semantic_inline.explanation.summary.is_empty()
    {
        stop_runtime(&shutdown, join)?;
        return Err(
            format!("unexpected semantic inline-vector response: {semantic_inline:?}").into(),
        );
    }

    let hybrid_inline = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .search()
                .hybrid()
                .native("sphinx")
                .semantic_text("quartz")
                .pinned(pin())
                .top_k(2)
                .execute()
        },
        |response| response.generation == pin() && !response.results.is_empty(),
    )?;
    let hybrid_inline_top = hybrid_inline
        .results
        .first()
        .ok_or_else(|| "missing hybrid inline-vector candidate".to_string())?;
    if hybrid_inline.generation != pin()
        || hybrid_inline_top.candidate_id != "alpha"
        || hybrid_inline.explanation.summary.is_empty()
    {
        stop_runtime(&shutdown, join)?;
        return Err(format!("unexpected hybrid inline-vector response: {hybrid_inline:?}").into());
    }

    stop_runtime(&shutdown, join)
}

#[test]
fn sdk_multi_generation_restart_frontdoor_preserves_pinned_and_flips_active_per_track() -> TestResult
{
    let complex_timeout = Duration::from_secs(30);
    let dir = tempfile::tempdir()?;
    let state_root = dir.path().to_path_buf();
    let (client, shutdown, join) =
        start_sdk_frontdoor_runtime_at_state_root(&state_root, "sdk-frontdoor-multigen-v1")?;

    let _lexical_receipt_v1 = client.lexical().publish(&lexical_batch()?)?;
    let _structural_receipt_v1 = client.structural().publish(&structural_batch()?)?;
    let _activation_v1 = wait_for_sdk_ready(complex_timeout, || {
        client
            .generations()
            .activate()
            .repo(repo())
            .revision(revision())
            .generation(generation())
            .manifest_digest("manifest:v1-active")
            .tracks([SearchPlaneTrackKind::Lexical])?
            .commit()
    })?;

    let lexical_active_v1 = wait_for_sdk_observation(
        complex_timeout,
        || {
            client
                .lexical()
                .query()
                .native("todo")
                .active(repo(), revision())
                .top_k(2)
                .execute()
        },
        |response| response.generation == pin() && response.results.len() == 1,
    )?;
    let lexical_active_v1_candidate = lexical_active_v1
        .results
        .first()
        .ok_or_else(|| "missing active v1 lexical candidate".to_string())?;
    if lexical_active_v1.generation != pin()
        || lexical_active_v1_candidate.candidate_id != "chunk-dirty"
    {
        stop_runtime(&shutdown, join)?;
        return Err(format!("unexpected active v1 lexical response: {lexical_active_v1:?}").into());
    }

    let _lexical_receipt_v2 = client.lexical().publish(&lexical_batch_two()?)?;
    let _structural_receipt_v2 = client.structural().publish(&structural_batch_two()?)?;

    let lexical_pinned_v2 = wait_for_sdk_observation(
        complex_timeout,
        || {
            client
                .lexical()
                .query()
                .native("todo_v2")
                .pinned(pin_two())
                .top_k(2)
                .execute()
        },
        |response| response.generation == pin_two() && response.results.len() == 1,
    )?;
    let lexical_pinned_v2_candidate = lexical_pinned_v2
        .results
        .first()
        .ok_or_else(|| "missing pinned v2 lexical candidate".to_string())?;
    if lexical_pinned_v2.generation != pin_two()
        || lexical_pinned_v2_candidate.candidate_id != "chunk-dirty-v2"
    {
        stop_runtime(&shutdown, join)?;
        return Err(format!("unexpected pinned v2 lexical response: {lexical_pinned_v2:?}").into());
    }

    let semantic_pinned_v2 = wait_for_sdk_observation(
        complex_timeout,
        || {
            client
                .semantic()
                .query()
                .text("obsidian")
                .pinned(pin_two())
                .top_k(2)
                .execute()
        },
        |response| response.generation == pin_two() && !response.results.is_empty(),
    )?;
    let semantic_pinned_v2_top = semantic_pinned_v2
        .results
        .first()
        .ok_or_else(|| "missing pinned v2 semantic candidate".to_string())?;
    if semantic_pinned_v2.generation != pin_two() || semantic_pinned_v2_top.candidate_id != "gamma"
    {
        stop_runtime(&shutdown, join)?;
        return Err(
            format!("unexpected pinned v2 semantic response: {semantic_pinned_v2:?}").into(),
        );
    }

    let structural_pinned_v2 = wait_for_sdk_observation(
        complex_timeout,
        || {
            client
                .structural()
                .query()
                .native("match { function_item }")
                .pinned(pin_two())
                .top_k(2)
                .execute()
        },
        |response| response.generation == pin_two() && response.results.len() == 1,
    )?;
    let structural_pinned_v2_candidate = structural_pinned_v2
        .results
        .first()
        .ok_or_else(|| "missing pinned v2 structural candidate".to_string())?;
    if structural_pinned_v2.generation != pin_two()
        || structural_pinned_v2_candidate.candidate_id != "chunk-tree-v2"
    {
        stop_runtime(&shutdown, join)?;
        return Err(
            format!("unexpected pinned v2 structural response: {structural_pinned_v2:?}").into(),
        );
    }

    stop_runtime(&shutdown, join)?;
    let (client, shutdown, join) =
        start_sdk_frontdoor_runtime_at_state_root(&state_root, "sdk-frontdoor-multigen-v2")?;

    let lexical_active_after_restart = wait_for_sdk_observation(
        complex_timeout,
        || {
            client
                .lexical()
                .query()
                .native("todo")
                .active(repo(), revision())
                .top_k(2)
                .execute()
        },
        |response| response.generation == pin() && response.results.len() == 1,
    )?;
    let lexical_active_after_restart_candidate = lexical_active_after_restart
        .results
        .first()
        .ok_or_else(|| "missing restarted active v1 lexical candidate".to_string())?;
    if lexical_active_after_restart.generation != pin()
        || lexical_active_after_restart_candidate.candidate_id != "chunk-dirty"
    {
        stop_runtime(&shutdown, join)?;
        return Err(format!(
            "unexpected restarted active v1 lexical response: {lexical_active_after_restart:?}"
        )
        .into());
    }

    let lexical_pinned_v2_after_restart = wait_for_sdk_observation(
        complex_timeout,
        || {
            client
                .lexical()
                .query()
                .native("todo_v2")
                .pinned(pin_two())
                .top_k(2)
                .execute()
        },
        |response| response.generation == pin_two() && response.results.len() == 1,
    )?;
    let lexical_pinned_v2_after_restart_candidate = lexical_pinned_v2_after_restart
        .results
        .first()
        .ok_or_else(|| "missing restarted pinned v2 lexical candidate".to_string())?;
    if lexical_pinned_v2_after_restart.generation != pin_two()
        || lexical_pinned_v2_after_restart_candidate.candidate_id != "chunk-dirty-v2"
    {
        stop_runtime(&shutdown, join)?;
        return Err(format!(
            "unexpected restarted pinned v2 lexical response: \
             {lexical_pinned_v2_after_restart:?}"
        )
        .into());
    }

    let semantic_pinned_v2_after_restart = wait_for_sdk_observation_with_retry_codes(
        complex_timeout,
        &["NOT_READY"],
        || {
            client
                .semantic()
                .query()
                .text("obsidian")
                .pinned(pin_two())
                .top_k(2)
                .execute()
        },
        |response| response.generation == pin_two() && !response.results.is_empty(),
    )?;
    let semantic_pinned_v2_after_restart_top = semantic_pinned_v2_after_restart
        .results
        .first()
        .ok_or_else(|| "missing restarted pinned v2 semantic candidate".to_string())?;
    if semantic_pinned_v2_after_restart.generation != pin_two()
        || semantic_pinned_v2_after_restart_top.candidate_id != "gamma"
    {
        stop_runtime(&shutdown, join)?;
        return Err(format!(
            "unexpected restarted pinned v2 semantic response: \
             {semantic_pinned_v2_after_restart:?}"
        )
        .into());
    }

    let structural_pinned_v2_after_restart = wait_for_sdk_observation(
        complex_timeout,
        || {
            client
                .structural()
                .query()
                .native("match { function_item }")
                .pinned(pin_two())
                .top_k(2)
                .execute()
        },
        |response| response.generation == pin_two() && response.results.len() == 1,
    )?;
    let structural_pinned_v2_after_restart_candidate = structural_pinned_v2_after_restart
        .results
        .first()
        .ok_or_else(|| "missing restarted pinned v2 structural candidate".to_string())?;
    if structural_pinned_v2_after_restart.generation != pin_two()
        || structural_pinned_v2_after_restart_candidate.candidate_id != "chunk-tree-v2"
    {
        stop_runtime(&shutdown, join)?;
        return Err(format!(
            "unexpected restarted pinned v2 structural response: \
             {structural_pinned_v2_after_restart:?}"
        )
        .into());
    }

    let _activation_v2 = wait_for_sdk_ready(complex_timeout, || {
        client
            .generations()
            .activate()
            .repo(repo())
            .revision(revision())
            .generation(generation_two())
            .manifest_digest("manifest:v2-active")
            .tracks([SearchPlaneTrackKind::Lexical])?
            .commit()
    })?;

    let lexical_snapshot_v2 = wait_for_sdk_ready(complex_timeout, || {
        client
            .generations()
            .current(repo(), revision(), SearchPlaneTrackKind::Lexical)
    })?;
    if lexical_snapshot_v2.manifest_generation != generation_two()
        || lexical_snapshot_v2.manifest_digest != "manifest:v2-active"
    {
        stop_runtime(&shutdown, join)?;
        return Err(format!(
            "unexpected post-restart lexical generation snapshot: {lexical_snapshot_v2:?}"
        )
        .into());
    }

    let lexical_active_v2 = wait_for_sdk_observation(
        complex_timeout,
        || {
            client
                .lexical()
                .query()
                .native("todo_v2")
                .active(repo(), revision())
                .top_k(2)
                .execute()
        },
        |response| response.generation == pin_two() && response.results.len() == 1,
    )?;
    let lexical_active_v2_candidate = lexical_active_v2
        .results
        .first()
        .ok_or_else(|| "missing active v2 lexical candidate".to_string())?;
    if lexical_active_v2.generation != pin_two()
        || lexical_active_v2_candidate.candidate_id != "chunk-dirty-v2"
    {
        stop_runtime(&shutdown, join)?;
        return Err(format!("unexpected active v2 lexical response: {lexical_active_v2:?}").into());
    }

    let lexical_pinned_v1_after_flip = wait_for_sdk_observation(
        complex_timeout,
        || {
            client
                .lexical()
                .query()
                .native("todo")
                .pinned(pin())
                .top_k(2)
                .execute()
        },
        |response| response.generation == pin() && response.results.len() == 1,
    )?;
    let lexical_pinned_v1_after_flip_candidate = lexical_pinned_v1_after_flip
        .results
        .first()
        .ok_or_else(|| "missing pinned v1 lexical candidate after flip".to_string())?;
    if lexical_pinned_v1_after_flip.generation != pin()
        || lexical_pinned_v1_after_flip_candidate.candidate_id != "chunk-dirty"
    {
        stop_runtime(&shutdown, join)?;
        return Err(format!(
            "unexpected pinned v1 lexical response after flip: {lexical_pinned_v1_after_flip:?}"
        )
        .into());
    }

    stop_runtime(&shutdown, join)
}

#[test]
fn sdk_frontdoor_activation_builder_rejects_empty_tracks_before_wire_dispatch() -> TestResult {
    let (_dir, client, shutdown, join) = start_sdk_frontdoor_runtime("sdk-frontdoor-usage")?;

    expect_usage_error_contains(
        client
            .generations()
            .activate()
            .repo(repo())
            .revision(revision())
            .generation(generation())
            .manifest_digest("manifest:missing-tracks")
            .tracks([]),
        "at least one track",
    )?;

    stop_runtime(&shutdown, join)
}
