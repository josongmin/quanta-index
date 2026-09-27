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

use std::error::Error;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use quanta_index_contract::lex::{
    CommitRecord, CommitSha, LanguageCode, ParseNode, ParseRoleTag, ParseTreeRecord,
    compute_parse_tree_source_hash,
};
use quanta_index_contract::{
    BatchIngestMode, ChunkId, ChunkRecord, GenerationPin, HistoryIngestBatch, HistoryOrderV1,
    HistoryQueryRequest, HybridCandidateV1, HybridLaneV1, HybridQueryRequest, LqVisibility,
    ManifestGeneration, RepoId, RepoRelativePath, RevisionId, RuntimeCatalogIngestBatch,
    RuntimeChangedRecord, RuntimeDocFacetRecord, RuntimeEdgeAuthorityRecord,
    RuntimeMetadataQueryRequest, RuntimeSnapshotRecord, SearchPlaneIngestIpcRequest,
    SearchPlaneIngestIpcRequestEnvelope, SearchPlaneIngestIpcResponse,
    SearchPlaneIngestIpcResponseEnvelope, SearchPlaneQueryIpcRequest,
    SearchPlaneQueryIpcRequestEnvelope, SearchPlaneQueryIpcResponse,
    SearchPlaneQueryIpcResponseEnvelope, SearchScopeKey, SearchScopeSurface, SemanticQueryRequest,
    StructuralIngestBatch, StructuralQueryRequest, StructuralReplaceScope,
    StructuralTombstoneScope, StructuralTreeRecord, TextQueryRequest, TextQuerySyntax,
};
use quanta_index_ipc::send_request;
use quanta_index_searchd::app::SemanticEmbedderProfile;
use quanta_index_searchd::app::config::OpenAiEmbedderTuning;
use quanta_index_searchd_harness::{E2eRuntime, SourceCorpusFixture};
use serde::ser::{Serialize, SerializeStruct, Serializer};

use crate::frontdoor_scenarios::{
    IPC_FRONTDOOR_SCENARIOS, IpcFrontdoorExpectation, IpcFrontdoorSurface,
};

type TestResult = Result<(), Box<dyn Error>>;
static NEXT_REQUEST_ID: AtomicU64 = AtomicU64::new(1);
const READINESS_TIMEOUT: Duration = Duration::from_secs(15);

fn next_request_id() -> u64 {
    let id = NEXT_REQUEST_ID.fetch_add(1, Ordering::Relaxed);
    assert_ne!(id, 0, "test request IDs must remain nonzero");
    id
}

/// Harness-owned three-socket scenario fixture (TOPT-03: runtime fixture
/// ownership).
///
/// The daemon's tempdir, three-socket builder, and driver thread all live in
/// [`E2eRuntime`]: boot binds query/control/ingest and waits for all three,
/// and dropping the runtime performs the acknowledged lease-release (signal
/// the driver, join it — which drops the old runtime and releases the
/// state-root lease — before the tempdir is removed). Stale-socket cleanup
/// tolerates races (`NotFound` is not an error). Tests therefore return
/// `Err(..)` directly on failure paths with no manual shutdown/join
/// bookkeeping; teardown is owned by the harness.
struct ScenarioFixture {
    runtime: E2eRuntime,
    query_socket: std::path::PathBuf,
    ingest_socket: std::path::PathBuf,
}

impl ScenarioFixture {
    fn boot() -> Result<Self, Box<dyn Error>> {
        Self::boot_with_profile(SemanticEmbedderProfile::hash_dev())
    }

    fn boot_with_profile(profile: SemanticEmbedderProfile) -> Result<Self, Box<dyn Error>> {
        Self::boot_with_profile_and_grant(
            profile,
            quanta_index_searchd::app::config::ProviderEgressGrantConfig::default(),
        )
    }

    fn boot_with_profile_and_grant(
        profile: SemanticEmbedderProfile,
        grant: quanta_index_searchd::app::config::ProviderEgressGrantConfig,
    ) -> Result<Self, Box<dyn Error>> {
        let mut runtime =
            E2eRuntime::boot_with_embedder_profile(profile)?.with_provider_egress_grant(grant);
        // Eager start surfaces a boot refusal here and binds all three
        // sockets before any byte is published.
        runtime.start()?;
        let (query_socket, ingest_socket) = {
            let (query, _, ingest) = runtime
                .socket_paths()
                .ok_or_else(|| "fixture: driver started without socket paths".to_string())?;
            (query.to_path_buf(), ingest.to_path_buf())
        };
        Ok(Self {
            runtime,
            query_socket,
            ingest_socket,
        })
    }
}

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
    RepoId::new("repo-int").expect("static fixture ID satisfies canonical policy")
}

fn revision() -> RevisionId {
    RevisionId::new("rev-int").expect("static fixture ID satisfies canonical policy")
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

fn publish_structural_ready_fixture(socket: &Path, corpus: &mut SourceCorpusFixture) -> TestResult {
    publish_search_corpus_chunks(
        corpus,
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
    seal_lexical(socket, corpus)?;
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

fn scope_key(path: &str) -> SearchScopeKey {
    SearchScopeKey {
        doc_surface: SearchScopeSurface::Chunk,
        repo_relative_path: RepoRelativePath::new(path),
    }
}

fn dispatch_ingest(socket: &Path, payload: SearchPlaneIngestIpcRequest) -> TestResult {
    // Like every producer, stamp the canonical batch digest before sending
    // (QI-BB-032); the search plane refuses any other digest.
    let payload = quanta_index_searchd_harness::stamped_ingest_request(payload)?;
    let response = send_ingest_request(
        socket,
        &SearchPlaneIngestIpcRequestEnvelope {
            request_id: next_request_id(),
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
    corpus: &mut SourceCorpusFixture,
    chunks: Vec<ChunkRecord>,
    bundle_payload: Option<Vec<u8>>,
) -> TestResult {
    corpus.replace_chunks(chunks)?;
    if let Some(bytes) = bundle_payload {
        corpus.set_metadata(bytes)?;
    }
    Ok(())
}

fn tombstone_lexical_scopes(corpus: &mut SourceCorpusFixture, paths: &[&str]) -> TestResult {
    for path in paths {
        corpus.delete_path(path)?;
    }
    Ok(())
}

fn seal_lexical(socket: &Path, corpus: &mut SourceCorpusFixture) -> TestResult {
    corpus.publish(socket, next_request_id())?;
    Ok(())
}

fn publish_history_commits(socket: &Path, commits: Vec<CommitRecord>) -> TestResult {
    dispatch_ingest(
        socket,
        SearchPlaneIngestIpcRequest::PublishHistoryBatch(HistoryIngestBatch {
            repo_id: repo(),
            revision_id: revision(),
            generation: generation(),
            manifest_digest: Some(format!("e2e-history-manifest-{}", generation().get())),
            batch_digest: String::new(),
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
            batch_digest: String::new(),
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
            batch_digest: String::new(),
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
            batch_digest: String::new(),
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
            batch_digest: String::new(),
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
            batch_digest: String::new(),
            mode: BatchIngestMode::ReplaceGeneration,
            replace_scopes: Vec::new(),
            tombstone_scopes: Vec::new(),
            seal: true,
        }),
    )
}

fn verify_publish_dispatch_lexical_roundtrip(socket: &Path) -> TestResult {
    let mut ready_candidates = None;
    if !wait_until(READINESS_TIMEOUT, || {
        let probe = lex_query("hello");
        match send_query_request(socket, &probe) {
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
                | SearchPlaneQueryIpcResponse::ActiveGenerationSnapshot(_)
                | SearchPlaneQueryIpcResponse::ResolvedLexicalGeneration(_)
                | SearchPlaneQueryIpcResponse::Error(_) => false,
            },
            Err(_) => false,
        }
    }) {
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
    Ok(())
}

fn verify_publish_dispatch_sourcegraph_roundtrip(socket: &Path) -> TestResult {
    let pin = GenerationPin::new(repo(), revision(), generation());
    let response = send_query_request(
        socket,
        &SearchPlaneQueryIpcRequestEnvelope {
            request_id: 44,
            payload: SearchPlaneQueryIpcRequest::Text(TextQueryRequest {
                syntax: TextQuerySyntax::Sourcegraph,
                query_text: "hello".to_string(),
                constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                generation: Some(pin.clone()),
                generation_selector: None,
                top_k: 50,
                cursor: None,
            }),
        },
    )?;
    let sourcegraph = match response.payload {
        SearchPlaneQueryIpcResponse::Text(payload) => payload,
        other => return Err(format!("expected Text, got {other:?}").into()),
    };
    if sourcegraph.generation != pin {
        return Err("sourcegraph response generation did not echo request pin".into());
    }
    let ids: Vec<String> = sourcegraph
        .results
        .iter()
        .map(|candidate| candidate.candidate_id.clone())
        .collect();
    if !ids.iter().any(|id| id == "c1") || !ids.iter().any(|id| id == "c2") {
        return Err(format!("missing expected sourcegraph ids: {ids:?}").into());
    }
    Ok(())
}

#[test]
fn publish_dispatch_queries_share_one_indexed_fixture() -> TestResult {
    let fixture = ScenarioFixture::boot()?;
    let socket = fixture.query_socket.clone();
    let ingest_socket = fixture.ingest_socket;
    let mut corpus = SourceCorpusFixture::new(
        repo(),
        revision(),
        generation(),
        format!("fixture-event-{}", next_request_id()),
    );
    publish_search_corpus_chunks(
        &mut corpus,
        vec![
            chunk_record("c1", "hello world")?,
            chunk_record("c2", "hello rust")?,
            chunk_record("c3", "goodbye")?,
        ],
        Some(b"manifest".to_vec()),
    )?;
    seal_lexical(&ingest_socket, &mut corpus)?;

    let verification: TestResult = (|| {
        verify_publish_dispatch_lexical_roundtrip(&socket)
            .map_err(|error| -> Box<dyn Error> { format!("lexical: {error}").into() })?;
        verify_publish_dispatch_sourcegraph_roundtrip(&socket)
            .map_err(|error| -> Box<dyn Error> { format!("sourcegraph: {error}").into() })?;
        Ok(())
    })();
    verification
}

#[test]
fn sourcegraph_path_and_lang_filters_execute_against_indexed_metadata() -> TestResult {
    let fixture = ScenarioFixture::boot()?;
    let socket = fixture.query_socket.clone();
    let ingest_socket = fixture.ingest_socket;
    let mut corpus = SourceCorpusFixture::new(
        repo(),
        revision(),
        generation(),
        format!("fixture-event-{}", next_request_id()),
    );
    publish_search_corpus_chunks(
        &mut corpus,
        vec![
            chunk_record_with_metadata("alpha", "src/lib.rs", "rust", 3, 8, "needle alpha")?,
            chunk_record_with_metadata("beta", "src/main.rs", "rust", 10, 18, "needle beta")?,
            chunk_record_with_metadata("gamma", "src/lib.py", "python", 20, 24, "needle gamma")?,
        ],
        Some(b"manifest".to_vec()),
    )?;
    seal_lexical(&ingest_socket, &mut corpus)?;

    let req = SearchPlaneQueryIpcRequestEnvelope {
        request_id: 7,
        payload: SearchPlaneQueryIpcRequest::Text(TextQueryRequest {
            syntax: TextQuerySyntax::Sourcegraph,
            query_text: "path:src/lib.rs lang:rust needle".to_string(),
            constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
            generation: Some(GenerationPin::new(repo(), revision(), generation())),
            generation_selector: None,
            top_k: 50,
            cursor: None,
        }),
    };

    if !wait_until(READINESS_TIMEOUT, || {
        send_query_request(&socket, &req)
            .map(|resp| !matches!(resp.payload, SearchPlaneQueryIpcResponse::Error(_)))
            .unwrap_or(false)
    }) {
        return Err("sourcegraph metadata query never became ready".into());
    }

    let response = send_query_request(&socket, &req)?;
    let results = match response.payload {
        SearchPlaneQueryIpcResponse::Text(lexical) => lexical.results,
        other => {
            return Err(format!("expected Text, got {other:?}").into());
        }
    };
    if results.len() != 1 {
        return Err(format!("expected 1 metadata-filtered hit, got {results:?}").into());
    }
    let candidate = results
        .first()
        .ok_or_else(|| "metadata-filtered result missing first candidate".to_string())?;
    if candidate.candidate_id != "alpha" {
        return Err(format!("expected alpha, got {}", candidate.candidate_id).into());
    }
    if candidate.repo_relative_path.as_str() != "src/lib.rs" {
        return Err(format!(
            "expected src/lib.rs path, got {}",
            candidate.repo_relative_path.as_str()
        )
        .into());
    }
    if candidate.start_line != 3 || candidate.end_line != 8 {
        return Err(format!(
            "expected line span 3..8, got {}..{}",
            candidate.start_line, candidate.end_line
        )
        .into());
    }

    Ok(())
}

fn verify_history_generation_not_ready(socket: &Path) -> TestResult {
    let err = wait_for_typed_error(socket, &history_query("type:commit fix"), READINESS_TIMEOUT)?;
    if err.code.as_wire_str() != "HISTORY_GENERATION_NOT_READY" {
        return Err(format!("expected HISTORY_GENERATION_NOT_READY, got {}", err.code).into());
    }
    if !err.message.contains("not yet materialized") {
        return Err(format!("unexpected generation-not-ready message: {}", err.message).into());
    }
    Ok(())
}

fn verify_history_producer_unavailable_without_lexical_fallback(socket: &Path) -> TestResult {
    if !wait_until(READINESS_TIMEOUT, || {
        send_query_request(socket, &lex_query("fix"))
            .map(|resp| matches!(resp.payload, SearchPlaneQueryIpcResponse::Text(_)))
            .unwrap_or(false)
    }) {
        return Err("lexical fixture never became queryable".into());
    }

    let err = wait_for_typed_error(socket, &history_query("type:commit fix"), READINESS_TIMEOUT)?;
    if err.code.as_wire_str() != "HISTORY_PRODUCER_UNAVAILABLE" {
        return Err(format!("expected HISTORY_PRODUCER_UNAVAILABLE, got {}", err.code).into());
    }
    if !err.message.contains("producer data is unavailable") {
        return Err(format!("unexpected producer-unavailable message: {}", err.message).into());
    }
    Ok(())
}

#[test]
fn history_query_returns_typed_shard_unavailable_when_diff_shard_missing() -> TestResult {
    let fixture = ScenarioFixture::boot()?;
    let socket = fixture.query_socket.clone();
    let ingest_socket = fixture.ingest_socket;
    let mut corpus = SourceCorpusFixture::new(
        repo(),
        revision(),
        generation(),
        format!("fixture-event-{}", next_request_id()),
    );
    publish_search_corpus_chunks(
        &mut corpus,
        vec![chunk_record("history-lex", "history shard lexical proof")?],
        Some(b"manifest".to_vec()),
    )?;
    publish_history_commits(&ingest_socket, vec![history_commit_record()])?;
    seal_lexical(&ingest_socket, &mut corpus)?;
    if !wait_until(READINESS_TIMEOUT, || {
        send_query_request(&socket, &lex_query("history"))
            .map(|resp| matches!(resp.payload, SearchPlaneQueryIpcResponse::Text(_)))
            .unwrap_or(false)
    }) {
        return Err("history lexical proof never became ready".into());
    }

    let err = wait_for_typed_error(
        &socket,
        &history_query("type:diff history"),
        READINESS_TIMEOUT,
    )?;
    if err.code.as_wire_str() != "HISTORY_SHARD_UNAVAILABLE" {
        return Err(format!("expected HISTORY_SHARD_UNAVAILABLE, got {}", err.code).into());
    }
    if !err.message.contains("diff shard is unavailable") {
        return Err(format!("unexpected shard-unavailable message: {}", err.message).into());
    }

    Ok(())
}

#[test]
fn end_to_end_widened_history_and_runtime_queries_roundtrip_exact_truth() -> TestResult {
    let fixture = ScenarioFixture::boot()?;
    let socket = fixture.query_socket.clone();
    let ingest_socket = fixture.ingest_socket;
    let mut corpus = SourceCorpusFixture::new(
        repo(),
        revision(),
        generation(),
        format!("fixture-event-{}", next_request_id()),
    );
    publish_search_corpus_chunks(
        &mut corpus,
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
    seal_lexical(&ingest_socket, &mut corpus)?;
    publish_runtime_catalog_fixture(&ingest_socket)?;

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
                if err.code.as_wire_str() != expected_error.code
                    || !err.message.contains(expected_error.message_contains)
                {
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
                            SearchPlaneQueryIpcResponse::Error(err) => {
                                err.code.as_wire_str() != "NOT_READY"
                            }
                            _ => false,
                        })
                        .unwrap_or(false)
                }) {
                    return Err(format!("{} never became ready", scenario.name).into());
                }
                let response = send_query_request(&socket, &request)?;
                let history = match response.payload {
                    SearchPlaneQueryIpcResponse::History(history) => history,
                    other => {
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
                            SearchPlaneQueryIpcResponse::Error(err) => {
                                err.code.as_wire_str() != "NOT_READY"
                            }
                            _ => false,
                        })
                        .unwrap_or(false)
                }) {
                    return Err(format!("{} never became ready", scenario.name).into());
                }
                let response = send_query_request(&socket, &request)?;
                let history = match response.payload {
                    SearchPlaneQueryIpcResponse::History(history) => history,
                    other => {
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
                            SearchPlaneQueryIpcResponse::Error(err) => {
                                err.code.as_wire_str() != "NOT_READY"
                            }
                            _ => false,
                        })
                        .unwrap_or(false)
                }) {
                    return Err(format!("{} never became ready", scenario.name).into());
                }
                let response = send_query_request(&socket, &request)?;
                let runtime = match response.payload {
                    quanta_index_contract::SearchPlaneQueryIpcResponse::RuntimeMetadata(
                        runtime,
                    ) => runtime,
                    other => {
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
                    return Err(format!(
                        "{} runtime candidate drifted: expected {:?}, got {:?}",
                        scenario.name, expected, observed
                    )
                    .into());
                }
            }
        }
    }

    Ok(())
}

fn verify_hybrid_requires_joint_materialization(socket: &Path) -> TestResult {
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
                cursor: None,
            },
            semantic_query_text: "only".to_string(),
            generation: Some(GenerationPin::new(repo(), revision(), generation())),
            generation_selector: None,
            top_k: 5,
        }),
    };
    let response = send_query_request(socket, &hybrid_req)?;
    let err = match response.payload {
        SearchPlaneQueryIpcResponse::Error(err) => err,
        other => return Err(format!("expected Error, got {other:?}").into()),
    };
    if err.code.as_wire_str() != "NOT_READY" {
        return Err(format!("expected NOT_READY, got {}", err.code).into());
    }
    Ok(())
}

#[test]
fn hybrid_query_succeeds_when_both_tracks_sealed() -> TestResult {
    let fixture = ScenarioFixture::boot()?;
    let socket = fixture.query_socket.clone();
    let ingest_socket = fixture.ingest_socket;
    let mut corpus = SourceCorpusFixture::new(
        repo(),
        revision(),
        generation(),
        format!("fixture-event-{}", next_request_id()),
    );
    let alpha = chunk_record("alpha", "sphinx quartz")?;
    let beta = chunk_record("beta", "sphinx riddles")?;
    publish_search_corpus_chunks(&mut corpus, vec![alpha, beta], None)?;
    seal_lexical(&ingest_socket, &mut corpus)?;
    let pin = GenerationPin::new(repo(), revision(), generation());
    // Wait for joint lexical/semantic materialization from search-corpus ingest.
    if !wait_until(READINESS_TIMEOUT, || {
        let req = SearchPlaneQueryIpcRequestEnvelope {
            request_id: next_request_id(),
            payload: SearchPlaneQueryIpcRequest::Hybrid(HybridQueryRequest {
                text_query: TextQueryRequest {
                    syntax: TextQuerySyntax::Sourcegraph,
                    query_text: "sphinx".to_string(),
                    constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                    generation: Some(pin.clone()),
                    generation_selector: None,
                    top_k: 50,
                    cursor: None,
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
                cursor: None,
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
            return Err(format!("expected Hybrid, got {other:?}").into());
        }
    };
    if candidates.is_empty() {
        return Err("hybrid returned no candidates".into());
    }
    let top_id = candidates
        .first()
        .map(|row| row.candidate.candidate_id.clone())
        .unwrap_or_default();
    if top_id != "alpha" {
        return Err(format!("expected alpha top, got {top_id}").into());
    }

    Ok(())
}

fn verify_hybrid_generation_pin_mismatch(socket: &Path) -> TestResult {
    let response = send_query_request(
        socket,
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
                    cursor: None,
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
        other => return Err(format!("expected Error, got {other:?}").into()),
    };
    if err.code.as_wire_str() != "INVALID_REQUEST" {
        return Err(format!("expected INVALID_REQUEST, got {}", err.code).into());
    }
    if !err
        .message
        .contains("hybrid: lexical generation does not match semantic generation")
    {
        return Err(format!("unexpected mismatch message: {}", err.message).into());
    }
    Ok(())
}

fn verify_sourcegraph_context_filter(socket: &Path) -> TestResult {
    let req = SearchPlaneQueryIpcRequestEnvelope {
        request_id: 17,
        payload: SearchPlaneQueryIpcRequest::Text(TextQueryRequest {
            syntax: TextQuerySyntax::Sourcegraph,
            query_text: "fork:no archived:no visibility:public context:global needle".to_string(),
            constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
            generation: Some(GenerationPin::new(repo(), revision(), generation())),
            generation_selector: None,
            top_k: 50,
            cursor: None,
        }),
    };

    if !wait_until(READINESS_TIMEOUT, || {
        send_query_request(socket, &req)
            .map(|resp| match resp.payload {
                SearchPlaneQueryIpcResponse::Error(err) => err.code.as_wire_str() != "NOT_READY",
                _ => true,
            })
            .unwrap_or(false)
    }) {
        return Err("sourcegraph context filter never progressed past NOT_READY".into());
    }

    let response = send_query_request(socket, &req)?;
    let results = match response.payload {
        SearchPlaneQueryIpcResponse::Text(text) => text.results,
        other => return Err(format!("expected Text, got {other:?}").into()),
    };
    if results.len() != 1 {
        return Err(format!("expected one context-filtered hit, got {results:?}").into());
    }
    if results
        .first()
        .map(|candidate| candidate.candidate_id.as_str())
        != Some("alpha")
    {
        return Err(format!("expected alpha hit, got {results:?}").into());
    }
    Ok(())
}

fn verify_hybrid_visibility_filter(socket: &Path) -> TestResult {
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
                cursor: None,
            },
            semantic_query_text: "needle".to_string(),
            generation: Some(pin),
            generation_selector: None,
            top_k: 1,
        }),
    };

    if !wait_until(READINESS_TIMEOUT, || {
        send_query_request(socket, &req)
            .map(|resp| match resp.payload {
                SearchPlaneQueryIpcResponse::Error(err) => err.code.as_wire_str() != "NOT_READY",
                _ => true,
            })
            .unwrap_or(false)
    }) {
        return Err("hybrid visibility filter never progressed past NOT_READY".into());
    }

    let response = send_query_request(socket, &req)?;
    let results = match response.payload {
        SearchPlaneQueryIpcResponse::Hybrid(hybrid) => hybrid.results,
        other => return Err(format!("expected Hybrid, got {other:?}").into()),
    };
    if results.len() != 1 {
        return Err(format!("expected one hybrid hit, got {results:?}").into());
    }
    if results
        .first()
        .map(|row| row.candidate.candidate_id.as_str())
        != Some("alpha")
    {
        return Err(format!("expected alpha top hit, got {results:?}").into());
    }
    Ok(())
}

#[test]
fn repo_metadata_filters_share_one_indexed_fixture() -> TestResult {
    let fixture = ScenarioFixture::boot()?;
    let socket = fixture.query_socket.clone();
    let ingest_socket = fixture.ingest_socket;
    let mut corpus = SourceCorpusFixture::new(
        repo(),
        revision(),
        generation(),
        format!("fixture-event-{}", next_request_id()),
    );
    publish_search_corpus_chunks(
        &mut corpus,
        vec![chunk_record("alpha", "needle")?],
        Some(repo_metadata_payload(
            false,
            false,
            LqVisibility::Public,
            &["global", "team-search"],
        )?),
    )?;
    seal_lexical(&ingest_socket, &mut corpus)?;

    let verification: TestResult = (|| {
        let verify_sourcegraph_context_filter_fn: fn(&Path) -> TestResult =
            verify_sourcegraph_context_filter;
        for (name, verify) in [
            (
                "sourcegraph_context_filter",
                verify_sourcegraph_context_filter_fn,
            ),
            ("hybrid_visibility_filter", verify_hybrid_visibility_filter),
        ] {
            verify(&socket)
                .map_err(|error| -> Box<dyn Error> { format!("{name}: {error}").into() })?;
        }
        Ok(())
    })();
    verification
}

fn verify_semantic_requires_materialization(socket: &Path) -> TestResult {
    let req = SearchPlaneQueryIpcRequestEnvelope {
        request_id: next_request_id(),
        payload: SearchPlaneQueryIpcRequest::Semantic(SemanticQueryRequest {
            query_text: "semantic".to_string(),
            constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
            generation: Some(GenerationPin::new(repo(), revision(), generation())),
            generation_selector: None,
            lexical_scope: None,
            top_k: 3,
        }),
    };
    let response = send_query_request(socket, &req)?;
    let err = match response.payload {
        SearchPlaneQueryIpcResponse::Error(e) => e,
        other => return Err(format!("expected Error, got {other:?}").into()),
    };
    if err.code.as_wire_str() != "SEMANTIC_GENERATION_NOT_MATERIALIZED" {
        return Err(format!(
            "expected SEMANTIC_GENERATION_NOT_MATERIALIZED, got {}",
            err.code
        )
        .into());
    }
    Ok(())
}

fn verify_semantic_without_lexical_scope(socket: &Path) -> TestResult {
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
        send_query_request(socket, &req)
            .map(|resp| !matches!(resp.payload, SearchPlaneQueryIpcResponse::Error(_)))
            .unwrap_or(false)
    }) {
        return Err("semantic no-scope query never became ready".into());
    }

    let response = send_query_request(socket, &req)?;
    let results = match response.payload {
        SearchPlaneQueryIpcResponse::Semantic(semantic) => {
            if semantic.generation != pin {
                return Err("semantic response generation did not echo request pin".into());
            }
            semantic.results
        }
        other => return Err(format!("expected Semantic, got {other:?}").into()),
    };
    let ids: Vec<String> = results
        .iter()
        .map(|candidate| candidate.candidate_id.clone())
        .collect();
    if ids != ["alpha".to_string()] {
        return Err(format!("expected global nearest [alpha], got {ids:?}").into());
    }
    Ok(())
}

/// Boot the real daemon with an explicit `OpenAi` embedder profile.
/// Manual release proof (gated, real `OpenAI` API).
///
/// Full daemon -> corpus embed -> lancedb cosine -> ranked results. The query
/// shares NO meaningful token with either indexed doc (only the stopword
/// "the"), so a token-distribution hash embedder (the prior FNV-1a default)
/// cannot rank them by meaning. Real neural embeddings must rank the
/// semantically-related "cat" doc above the unrelated "finance" doc. This is
/// the end-to-end capability that was structurally impossible before.
///
/// `#[ignore]` because it hits the real `OpenAI` API. The operator must set
/// `OPENAI_API_KEY` and the `QUANTA_INDEX_PROVIDER_{TENANT,ENDPOINT,REGION,
/// RETENTION,PROFILE,SOURCE_CONTENT_CONSENT}` grant fields before running:
/// `OPENAI_API_KEY=<key> ./scripts/cargow --lane test-daemon-lane test \
///   -p quanta-index-searchd-runtime --test runtime_fast_suite \
///   end_to_end::openai_semantic_paraphrase_outranks_unrelated_v1 \
///   -- --ignored --nocapture`
#[test]
#[ignore = "hits the real OpenAI API; run with OPENAI_API_KEY set and --ignored"]
fn openai_semantic_paraphrase_outranks_unrelated_v1() -> TestResult {
    let api_key = match std::env::var("OPENAI_API_KEY") {
        Ok(key) if !key.trim().is_empty() => key,
        _ => return Err("OPENAI_API_KEY must be set to run this gated test".into()),
    };

    let fixture = ScenarioFixture::boot_with_profile_and_grant(
        SemanticEmbedderProfile::OpenAi {
            model: "text-embedding-3-small".to_string(),
            model_revision: "live".to_string(),
            dimension: 1536,
            api_key,
            tuning: OpenAiEmbedderTuning::default(),
        },
        quanta_index_searchd::app::config::ProviderEgressGrantConfig::from_env()?,
    )?;
    let socket = fixture.query_socket.clone();
    let ingest_socket = fixture.ingest_socket;
    let mut corpus = SourceCorpusFixture::new(
        repo(),
        revision(),
        generation(),
        format!("fixture-event-{}", next_request_id()),
    );

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
    publish_search_corpus_chunks(&mut corpus, vec![cat_doc, finance_doc], None)?;
    seal_lexical(&ingest_socket, &mut corpus)?;

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
        return Err("openai semantic query never became ready".into());
    }

    let response = send_query_request(&socket, &req)?;
    let results = match response.payload {
        SearchPlaneQueryIpcResponse::Semantic(semantic) => {
            if semantic.generation != pin {
                return Err("openai semantic response did not echo request pin".into());
            }
            semantic.results
        }
        other => {
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
        return Err(format!("expected both docs ranked, got {ranked:?}").into());
    }
    if ranked.first().map(String::as_str) != Some("cat-doc") {
        return Err(format!(
            "neural ranking failed: a paraphrase query should rank 'cat-doc' first; got {ranked:?}"
        )
        .into());
    }
    if ranked.get(1).map(String::as_str) != Some("finance-doc") {
        return Err(
            format!("expected unrelated 'finance-doc' ranked second, got {ranked:?}").into(),
        );
    }

    Ok(())
}

fn verify_semantic_search_owned_text_derivation(socket: &Path) -> TestResult {
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
        send_query_request(socket, &req)
            .map(|resp| !matches!(resp.payload, SearchPlaneQueryIpcResponse::Error(_)))
            .unwrap_or(false)
    }) {
        return Err("semantic default text query never became ready".into());
    }

    let response = send_query_request(socket, &req)?;
    let semantic = match response.payload {
        SearchPlaneQueryIpcResponse::Semantic(semantic) => semantic,
        other => return Err(format!("expected Semantic, got {other:?}").into()),
    };
    let first = semantic
        .results
        .first()
        .ok_or_else(|| "semantic default derivation returned no results".to_string())?;
    if first.candidate_id != "derivation-alpha" {
        return Err(format!(
            "expected derivation-alpha candidate, got {}",
            first.candidate_id
        )
        .into());
    }
    Ok(())
}

#[test]
fn semantic_query_uses_search_owned_text_derivation_with_explicit_hash_profile() -> TestResult {
    // The harness default embedder is the explicit hash profile at the
    // search-owned dimension, so plain boot is this test's fixture.
    let fixture = ScenarioFixture::boot()?;
    let socket = fixture.query_socket.clone();
    let ingest_socket = fixture.ingest_socket;
    let mut corpus = SourceCorpusFixture::new(
        repo(),
        revision(),
        generation(),
        format!("fixture-event-{}", next_request_id()),
    );

    publish_search_corpus_chunks(
        &mut corpus,
        vec![
            chunk_record("alpha", "parser pipeline typed semantic search")?,
            chunk_record("beta", "archive storage compaction")?,
        ],
        None,
    )?;
    seal_lexical(&ingest_socket, &mut corpus)?;

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
        return Err("semantic explicit hash query never became ready".into());
    }

    let response = send_query_request(&socket, &req)?;
    let semantic = match response.payload {
        SearchPlaneQueryIpcResponse::Semantic(semantic) => semantic,
        other => {
            return Err(format!("expected semantic response, got {other:?}").into());
        }
    };
    if semantic.generation != GenerationPin::new(repo(), revision(), generation()) {
        return Err("semantic explicit hash response did not echo request pin".into());
    }
    if !matches!(semantic.results.as_slice(), [only] if only.candidate_id == "alpha") {
        return Err(format!(
            "expected explicit-hash semantic query to rank alpha first, got {semantic:?}"
        )
        .into());
    }

    Ok(())
}

fn verify_semantic_generation_pin_mismatch(socket: &Path) -> TestResult {
    let response = send_query_request(
        socket,
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
                    cursor: None,
                }),
                top_k: 1,
            }),
        },
    )?;
    let err = match response.payload {
        SearchPlaneQueryIpcResponse::Error(err) => err,
        other => return Err(format!("expected Error, got {other:?}").into()),
    };
    if err.code.as_wire_str() != "INVALID_REQUEST" {
        return Err(format!("expected INVALID_REQUEST, got {}", err.code).into());
    }
    if !err
        .message
        .contains("semantic: scope generation does not match semantic request generation")
    {
        return Err(format!("unexpected mismatch message: {}", err.message).into());
    }
    Ok(())
}

fn verify_semantic_scoped_unindexed_lexical_scope(socket: &Path) -> TestResult {
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
                cursor: None,
            }),
            top_k: 1,
        }),
    };

    if !wait_until(READINESS_TIMEOUT, || {
        send_query_request(socket, &req)
            .map(|resp| !matches!(resp.payload, SearchPlaneQueryIpcResponse::Error(_)))
            .unwrap_or(false)
    }) {
        return Err("semantic unindexed scope query never became ready".into());
    }

    let response = send_query_request(socket, &req)?;
    let results = match response.payload {
        SearchPlaneQueryIpcResponse::Semantic(semantic) => {
            if semantic.generation != pin {
                return Err("semantic response generation did not echo request pin".into());
            }
            semantic.results
        }
        other => return Err(format!("expected Semantic, got {other:?}").into()),
    };
    let ids: Vec<String> = results
        .iter()
        .map(|candidate| candidate.candidate_id.clone())
        .collect();
    if ids != ["alpha".to_string()] {
        return Err(format!("expected scoped semantic intersection [alpha], got {ids:?}").into());
    }
    Ok(())
}

#[test]
fn semantic_query_with_lexical_scope_returns_intersection_only() -> TestResult {
    let fixture = ScenarioFixture::boot()?;
    let socket = fixture.query_socket.clone();
    let ingest_socket = fixture.ingest_socket;
    let mut corpus = SourceCorpusFixture::new(
        repo(),
        revision(),
        generation(),
        format!("fixture-event-{}", next_request_id()),
    );
    let pin = GenerationPin::new(repo(), revision(), generation());
    let alpha = chunk_record("alpha", "scope needle")?;
    let beta = chunk_record("beta", "scope miss")?;
    let gamma = chunk_record("gamma", "outside needle")?;
    publish_search_corpus_chunks(&mut corpus, vec![alpha, beta, gamma], None)?;
    seal_lexical(&ingest_socket, &mut corpus)?;

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
                cursor: None,
            }),
            top_k: 2,
        }),
    };

    if !wait_until(READINESS_TIMEOUT, || {
        send_query_request(&socket, &req)
            .map(|resp| !matches!(resp.payload, SearchPlaneQueryIpcResponse::Error(_)))
            .unwrap_or(false)
    }) {
        return Err("semantic scoped query never became ready".into());
    }

    let response = send_query_request(&socket, &req)?;
    let results = match response.payload {
        SearchPlaneQueryIpcResponse::Semantic(semantic) => semantic.results,
        other => {
            return Err(format!("expected Semantic, got {other:?}").into());
        }
    };
    let ids: Vec<String> = results
        .iter()
        .map(|candidate| candidate.candidate_id.clone())
        .collect();
    if ids != ["alpha".to_string()] {
        return Err(format!("expected scoped semantic intersection [alpha], got {ids:?}").into());
    }

    Ok(())
}

#[test]
fn semantic_scoped_query_ignores_out_of_scope_global_nearest_hit() -> TestResult {
    let fixture = ScenarioFixture::boot()?;
    let socket = fixture.query_socket.clone();
    let ingest_socket = fixture.ingest_socket;
    let mut corpus = SourceCorpusFixture::new(
        repo(),
        revision(),
        generation(),
        format!("fixture-event-{}", next_request_id()),
    );
    let pin = GenerationPin::new(repo(), revision(), generation());
    let alpha = chunk_record("alpha", "focus alpha")?;
    let beta = chunk_record("beta", "scope focus")?;
    let gamma = chunk_record("gamma", "scope gamma")?;
    publish_search_corpus_chunks(&mut corpus, vec![alpha, beta, gamma], None)?;
    seal_lexical(&ingest_socket, &mut corpus)?;

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
                cursor: None,
            }),
            top_k: 1,
        }),
    };

    if !wait_until(READINESS_TIMEOUT, || {
        send_query_request(&socket, &req)
            .map(|resp| !matches!(resp.payload, SearchPlaneQueryIpcResponse::Error(_)))
            .unwrap_or(false)
    }) {
        return Err("semantic scoped query never became ready".into());
    }

    let response = send_query_request(&socket, &req)?;
    let results = match response.payload {
        SearchPlaneQueryIpcResponse::Semantic(semantic) => semantic.results,
        other => {
            return Err(format!("expected Semantic, got {other:?}").into());
        }
    };
    let ids: Vec<String> = results
        .iter()
        .map(|candidate| candidate.candidate_id.clone())
        .collect();
    if ids != ["beta".to_string()] {
        return Err(format!("expected scoped semantic [beta], got {ids:?}").into());
    }

    Ok(())
}

fn verify_semantic_empty_text_refusal(socket: &Path) -> TestResult {
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
        send_query_request(socket, &req)
            .map(|resp| match resp.payload {
                SearchPlaneQueryIpcResponse::Error(err) => err.code.as_wire_str() != "NOT_READY",
                _ => true,
            })
            .unwrap_or(false)
    }) {
        return Err("semantic empty-query request never progressed past NOT_READY".into());
    }

    let response = send_query_request(socket, &req)?;
    let err = match response.payload {
        SearchPlaneQueryIpcResponse::Error(err) => err,
        other => return Err(format!("expected Error, got {other:?}").into()),
    };
    if err.code.as_wire_str() != "EMPTY_QUERY" {
        return Err(format!("expected EMPTY_QUERY, got {}", err.code).into());
    }

    Ok(())
}

#[test]
fn default_indexed_queries_share_one_fixture() -> TestResult {
    let fixture = ScenarioFixture::boot()?;
    let socket = fixture.query_socket.clone();
    let ingest_socket = fixture.ingest_socket;
    let mut corpus = SourceCorpusFixture::new(
        repo(),
        revision(),
        generation(),
        format!("fixture-event-{}", next_request_id()),
    );
    publish_search_corpus_chunks(
        &mut corpus,
        vec![
            chunk_record("alpha", "semantic alpha")?,
            chunk_record("beta", "semantic beta")?,
            chunk_record("derivation-alpha", "parser pipeline typed semantic search")?,
            chunk_record("derivation-beta", "archive storage compaction")?,
            chunk_record("history-fallback", "fix only lives in lexical content")?,
        ],
        Some(b"manifest".to_vec()),
    )?;
    seal_lexical(&ingest_socket, &mut corpus)?;

    let verification: TestResult = (|| {
        let verify_semantic_without_lexical_scope_fn: fn(&Path) -> TestResult =
            verify_semantic_without_lexical_scope;
        for (name, verify) in [
            (
                "without_lexical_scope",
                verify_semantic_without_lexical_scope_fn,
            ),
            (
                "scoped_unindexed_lexical_scope",
                verify_semantic_scoped_unindexed_lexical_scope,
            ),
            ("empty_text_refusal", verify_semantic_empty_text_refusal),
            (
                "search_owned_text_derivation",
                verify_semantic_search_owned_text_derivation,
            ),
            (
                "history_producer_unavailable_without_lexical_fallback",
                verify_history_producer_unavailable_without_lexical_fallback,
            ),
        ] {
            verify(&socket)
                .map_err(|error| -> Box<dyn Error> { format!("{name}: {error}").into() })?;
        }
        Ok(())
    })();
    verification
}

#[test]
fn semantic_query_fails_closed_when_runtime_has_no_query_embedder() -> TestResult {
    // The harness owns the degraded-config profile outright: no query-time
    // embedder, so semantic/hybrid queries fail closed while the corpus
    // still hash-derives and the generation materializes.
    let fixture = ScenarioFixture::boot_with_profile(SemanticEmbedderProfile::Unavailable)?;
    let socket = fixture.query_socket.clone();
    let ingest_socket = fixture.ingest_socket;
    let mut corpus = SourceCorpusFixture::new(
        repo(),
        revision(),
        generation(),
        format!("fixture-event-{}", next_request_id()),
    );

    let alpha = chunk_record("alpha", "semantic alpha")?;
    publish_search_corpus_chunks(&mut corpus, vec![alpha], None)?;
    seal_lexical(&ingest_socket, &mut corpus)?;

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
                SearchPlaneQueryIpcResponse::Error(err) => err.code.as_wire_str() != "NOT_READY",
                _ => true,
            })
            .unwrap_or(false)
    }) {
        return Err("semantic provider-unavailable query never progressed past NOT_READY".into());
    }

    let response = send_query_request(&socket, &req)?;
    let err = match response.payload {
        SearchPlaneQueryIpcResponse::Error(err) => err,
        other => {
            return Err(format!("expected Error, got {other:?}").into());
        }
    };
    if err.code.as_wire_str() != "SEM_PROVIDER_UNAVAILABLE" {
        return Err(format!("expected SEM_PROVIDER_UNAVAILABLE, got {}", err.code).into());
    }

    Ok(())
}

/// QI-BB-025: `top_k = 0` is refused under one code from a typed and from a
/// raw caller.
///
/// A typed hybrid request with `top_k = 0` does not even encode — the
/// client learns the shared code before any round trip — and raw bytes
/// carrying `top_k = 0` are answered typed by the daemon under that same
/// code, on the same connection.
fn verify_hybrid_zero_top_k_refusal(socket: &Path) -> TestResult {
    let hybrid = |top_k: u32| SearchPlaneQueryIpcRequestEnvelope {
        request_id: 44,
        payload: SearchPlaneQueryIpcRequest::Hybrid(HybridQueryRequest {
            text_query: TextQueryRequest {
                syntax: TextQuerySyntax::Sourcegraph,
                query_text: "needle".to_string(),
                constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                generation: Some(GenerationPin::new(repo(), revision(), generation())),
                generation_selector: None,
                top_k,
                cursor: None,
            },
            semantic_query_text: "needle".to_string(),
            generation: Some(GenerationPin::new(repo(), revision(), generation())),
            generation_selector: None,
            top_k,
        }),
    };
    // The client side: the typed request refuses to encode under the code.
    let encode_refusal = quanta_index_ipc::encode_request(&hybrid(0));
    match encode_refusal {
        Ok(_) => return Err("a hybrid request with top_k=0 must not encode".into()),
        Err(err)
            if err
                .to_string()
                .contains(quanta_index_contract::TOP_K_OUT_OF_RANGE_CODE) => {}
        Err(err) => {
            return Err(format!(
                "the encode refusal names {}: {err}",
                quanta_index_contract::TOP_K_OUT_OF_RANGE_CODE
            )
            .into());
        }
    }
    // The daemon side: bytes carrying top_k=0 that no encoder produced.
    let mut wire: ciborium::Value = {
        let mut buf = Vec::new();
        ciborium::into_writer(&hybrid(1), &mut buf)?;
        ciborium::from_reader(buf.as_slice())?
    };
    let patched = patch_every_top_k(&mut wire, 0);
    if patched == 0 {
        return Err("the wire carries a top_k to patch".into());
    }
    let refused = send_request::<ciborium::Value, SearchPlaneQueryIpcResponseEnvelope>(
        socket,
        &wire,
        quanta_index_ipc::ClientIoPolicy::default(),
    );
    let refused_code = match refused.map(|response| response.payload) {
        Ok(SearchPlaneQueryIpcResponse::Error(error)) => error.code,
        other => {
            return Err(
                format!("raw bytes with top_k=0 must be answered typed, got {other:?}").into(),
            );
        }
    };
    if refused_code.as_wire_str() != quanta_index_contract::TOP_K_OUT_OF_RANGE_CODE {
        return Err(format!(
            "the raw caller gets the typed client's code {}, got {refused_code}",
            quanta_index_contract::TOP_K_OUT_OF_RANGE_CODE
        )
        .into());
    }
    Ok(())
}

/// Replace every `top_k` entry in a decoded CBOR tree with `top_k`, so the
/// bytes carry a value no encoder would emit.
#[expect(
    clippy::wildcard_enum_match_arm,
    reason = "ciborium::Value is #[non_exhaustive]; the leaves and any future variant carry no top_k"
)]
fn patch_every_top_k(value: &mut ciborium::Value, top_k: u32) -> usize {
    match value {
        ciborium::Value::Map(fields) => fields
            .iter_mut()
            .map(|(key, entry)| {
                if matches!(key, ciborium::Value::Text(name) if name == "top_k") {
                    *entry = ciborium::Value::Integer(top_k.into());
                    1
                } else {
                    patch_every_top_k(entry, top_k)
                }
            })
            .sum(),
        ciborium::Value::Array(items) => items
            .iter_mut()
            .map(|item| patch_every_top_k(item, top_k))
            .sum(),
        // Every leaf variant, and — `ciborium::Value` being
        // `#[non_exhaustive]` — any variant this codec version does not
        // name, carries no `top_k`.
        _ => 0,
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
    let fixture = ScenarioFixture::boot()?;
    let socket = fixture.query_socket.clone();
    let ingest_socket = fixture.ingest_socket;
    let mut corpus = SourceCorpusFixture::new(
        repo(),
        revision(),
        generation(),
        format!("fixture-event-{}", next_request_id()),
    );
    // Lexical lane (`riddle`): beta only. Dense lane (`focus alpha`): alpha
    // first, beta second, gamma last. Fused at top_k=2: beta (both lanes),
    // then alpha on dense relevance alone; gamma, ranked last by the one
    // lane that saw it, stays out.
    let alpha = chunk_record("alpha", "focus alpha")?;
    let beta = chunk_record("beta", "riddle focus")?;
    let gamma = chunk_record("gamma", "scope gamma")?;
    publish_search_corpus_chunks(&mut corpus, vec![alpha, beta, gamma], None)?;
    seal_lexical(&ingest_socket, &mut corpus)?;

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
                cursor: None,
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
        return Err("hybrid query never became ready".into());
    }

    let response = send_query_request(&socket, &req)?;
    let results = match response.payload {
        SearchPlaneQueryIpcResponse::Hybrid(hybrid) => hybrid.results,
        other => {
            return Err(format!("expected Hybrid, got {other:?}").into());
        }
    };
    let ids: Vec<String> = results
        .iter()
        .map(|row| row.candidate.candidate_id.clone())
        .collect();
    if ids.first().map(String::as_str) != Some("beta") {
        return Err(format!("expected beta top, got {ids:?}").into());
    }
    if !ids.iter().any(|id| id == "alpha") {
        return Err(format!(
            "the dense-only relevant hit must enter the hybrid top-k, got {ids:?}"
        )
        .into());
    }
    if ids.iter().any(|id| id == "gamma") {
        return Err(format!(
            "a hit one lane ranked last must not outrank the fused pair at top_k=2: {ids:?}"
        )
        .into());
    }
    // QI-BB-022: each row says which lanes put it there. beta was seen by
    // both lanes (lexical rank 1, dense rank 2); alpha only by the dense
    // lane, at its rank 1. The fused score is the RRF of exactly those
    // ranks, recomputed here under the plane's k = 60.
    check_hybrid_lane_provenance(&results)?;

    Ok(())
}

/// The lanes that placed `id` in a fused list, with the rank each gave it.
fn lanes_of(
    results: &[HybridCandidateV1],
    id: &str,
) -> Result<Vec<(HybridLaneV1, u32)>, Box<dyn Error>> {
    results
        .iter()
        .find(|row| row.candidate.candidate_id == id)
        .map(|row| {
            row.contributions
                .iter()
                .map(|contribution| (contribution.lane, contribution.rank))
                .collect()
        })
        .ok_or_else(|| format!("{id} is in the fused list").into())
}

/// The provenance the admission fixture must carry.
///
/// beta is seen by both lanes, alpha by the dense lane only, every fused
/// score is the RRF of its own ranks, and every row's score is its
/// preferred lane's raw score.
fn check_hybrid_lane_provenance(results: &[HybridCandidateV1]) -> Result<(), Box<dyn Error>> {
    if lanes_of(results, "beta")? != [(HybridLaneV1::Lexical, 1), (HybridLaneV1::Dense, 2)] {
        return Err(format!("beta carries both lanes: {results:?}").into());
    }
    if lanes_of(results, "alpha")? != [(HybridLaneV1::Dense, 1)] {
        return Err(format!("alpha carries the dense lane only: {results:?}").into());
    }
    for row in results {
        let recomputed: f64 = row
            .contributions
            .iter()
            .map(|contribution| 1.0 / (60.0 + f64::from(contribution.rank)))
            .sum();
        if row.fused_score.to_bits() != recomputed.to_bits() {
            return Err(format!(
                "{} carries fused_score {} but its ranks sum to {recomputed}",
                row.candidate.candidate_id, row.fused_score
            )
            .into());
        }
        let Some(preferred) = row.contributions.first() else {
            return Err(format!("{} carries a lane", row.candidate.candidate_id).into());
        };
        if row.candidate.score.to_bits() != preferred.raw_score.to_bits() {
            return Err(format!(
                "{} carries its preferred lane's raw score: {row:?}",
                row.candidate.candidate_id
            )
            .into());
        }
    }
    Ok(())
}

#[test]
fn hybrid_query_repeated_tied_scope_query_keeps_stable_order() -> TestResult {
    let fixture = ScenarioFixture::boot()?;
    let socket = fixture.query_socket.clone();
    let ingest_socket = fixture.ingest_socket;
    let mut corpus = SourceCorpusFixture::new(
        repo(),
        revision(),
        generation(),
        format!("fixture-event-{}", next_request_id()),
    );
    let alpha = chunk_record("alpha", "scope tie")?;
    let beta = chunk_record("beta", "scope tie")?;
    publish_search_corpus_chunks(&mut corpus, vec![alpha, beta], None)?;
    seal_lexical(&ingest_socket, &mut corpus)?;

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
                cursor: None,
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
        return Err("hybrid tie determinism query never became ready".into());
    }

    let query_ids =
        |response: SearchPlaneQueryIpcResponseEnvelope| -> Result<Vec<String>, Box<dyn Error>> {
            match response.payload {
                SearchPlaneQueryIpcResponse::Hybrid(hybrid) => Ok(hybrid
                    .results
                    .into_iter()
                    .map(|row| row.candidate.candidate_id)
                    .collect()),
                other => Err(format!("expected Hybrid, got {other:?}").into()),
            }
        };

    let first_ids = query_ids(send_query_request(&socket, &request)?)?;
    let second_ids = query_ids(send_query_request(&socket, &request)?)?;
    if first_ids != second_ids {
        return Err(format!(
            "hybrid tie ordering drifted across repeated queries: first={first_ids:?} second={second_ids:?}"
        )
        .into());
    }
    if first_ids.len() != 2 {
        return Err(format!("expected 2 tied hybrid results, got {first_ids:?}").into());
    }

    Ok(())
}

fn verify_structural_generation_not_ready(socket: &Path) -> TestResult {
    let response = send_query_request(
        socket,
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
                    cursor: None,
                },
                cursor: None,
            }),
        },
    )?;
    let err = match response.payload {
        SearchPlaneQueryIpcResponse::Error(err) => err,
        other => return Err(format!("expected Error, got {other:?}").into()),
    };
    if err.code.as_wire_str() != "STR_GENERATION_NOT_READY" {
        return Err(format!("expected STR_GENERATION_NOT_READY, got {}", err.code).into());
    }
    if !err.message.contains("not yet materialized") {
        return Err(format!(
            "expected generation-not-ready structural message, got {}",
            err.message
        )
        .into());
    }
    Ok(())
}

#[test]
fn orphan_structural_tree_is_refused_before_query_readiness() -> TestResult {
    let fixture = ScenarioFixture::boot()?;
    let socket = fixture.query_socket.clone();
    let ingest_socket = fixture.ingest_socket;
    let mut corpus = SourceCorpusFixture::new(
        repo(),
        revision(),
        generation(),
        format!("fixture-event-{}", next_request_id()),
    );
    publish_search_corpus_chunks(
        &mut corpus,
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
    tombstone_lexical_scopes(&mut corpus, &["src/lib.rs"])?;
    seal_lexical(&ingest_socket, &mut corpus)?;
    let refused = publish_structural_scope(
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
    );
    let error = refused.expect_err("orphan structural tree must be refused at publication");
    if !error.to_string().contains("reason=source_chunk_missing") {
        return Err(format!("unexpected orphan-tree refusal: {error}").into());
    }
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
                cursor: None,
            },
            cursor: None,
        }),
    };
    let mut observed: Option<String> = None;
    let saw_expected = wait_until(READINESS_TIMEOUT, || {
        match send_query_request(&socket, &request) {
            Ok(response) => match response.payload {
                SearchPlaneQueryIpcResponse::Error(err) => {
                    observed = Some(err.code.as_wire_str().to_string());
                    err.code.as_wire_str() == "STR_GENERATION_NOT_READY"
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
        return Err(format!(
            "expected STR_GENERATION_NOT_READY after the orphan tree was refused, observed {observed:?}"
        )
        .into());
    }

    Ok(())
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
/// `verify_structural_generation_not_ready` above and
/// this looser wiring check ensures the composition root still emits a
/// typed structural code rather than panicking or returning a payload.
fn verify_structural_composition_wiring(socket: &Path) -> TestResult {
    let response = send_query_request(
        socket,
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
                    cursor: None,
                },
                cursor: None,
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
            let code = err.code.as_wire_str();
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

    result.map_err(Into::into)
}

fn verify_structural_sourcegraph_match(socket: &Path) -> TestResult {
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
                cursor: None,
            },
            cursor: None,
        }),
    };
    let mut observed: Option<String> = None;
    let saw_ready = wait_until(READINESS_TIMEOUT, || {
        match send_query_request(socket, &request) {
            Ok(response) => match response.payload {
                SearchPlaneQueryIpcResponse::Structural(structural) => {
                    observed = Some(format!("{structural:?}"));
                    structural.generation == pin && structural.results.len() == 1
                }
                SearchPlaneQueryIpcResponse::Error(err)
                    if err.code.as_wire_str() == "NOT_READY"
                        || err.code.as_wire_str() == "STR_GENERATION_NOT_READY" =>
                {
                    observed = Some(err.code.as_wire_str().to_string());
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
        return Err(format!(
            "structural Sourcegraph query never became ready; observed {observed:?}"
        )
        .into());
    }

    let response = send_query_request(socket, &request)?;
    let structural = match response.payload {
        SearchPlaneQueryIpcResponse::Structural(structural) => structural,
        other => return Err(format!("expected Structural, got {other:?}").into()),
    };
    if structural.generation != pin || structural.results.len() != 1 {
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
        return Err(
            format!("unexpected structural Sourcegraph candidate/binding: {candidate:?}").into(),
        );
    }
    Ok(())
}

fn verify_structural_sourcegraph_regex_match(socket: &Path) -> TestResult {
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
                cursor: None,
            },
            cursor: None,
        }),
    };
    let mut observed: Option<String> = None;
    let saw_ready = wait_until(READINESS_TIMEOUT, || {
        match send_query_request(socket, &request) {
            Ok(response) => match response.payload {
                SearchPlaneQueryIpcResponse::Structural(structural) => {
                    observed = Some(format!("{structural:?}"));
                    structural.generation == pin && structural.results.len() == 1
                }
                SearchPlaneQueryIpcResponse::Error(err)
                    if err.code.as_wire_str() == "NOT_READY"
                        || err.code.as_wire_str() == "STR_GENERATION_NOT_READY" =>
                {
                    observed = Some(err.code.as_wire_str().to_string());
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
        return Err(format!(
            "structural Sourcegraph regex query never became ready; observed {observed:?}"
        )
        .into());
    }

    let response = send_query_request(socket, &request)?;
    let structural = match response.payload {
        SearchPlaneQueryIpcResponse::Structural(structural) => structural,
        other => return Err(format!("expected Structural, got {other:?}").into()),
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
        return Err(format!(
            "unexpected structural Sourcegraph regex candidate/binding: {candidate:?}"
        )
        .into());
    }
    Ok(())
}

fn verify_structural_pattern_type_required(socket: &Path) -> TestResult {
    let response = send_query_request(
        socket,
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
                    cursor: None,
                },
                cursor: None,
            }),
        },
    )?;
    let err = match response.payload {
        SearchPlaneQueryIpcResponse::Error(err) => err,
        other => return Err(format!("expected Error, got {other:?}").into()),
    };
    if err.code.as_wire_str() != "BRIDGE_TRANSLATE_FAIL"
        || !err.message.contains("patterntype:structural")
    {
        return Err(format!(
            "expected BRIDGE_TRANSLATE_FAIL structural pattern-type error, got {err:?}"
        )
        .into());
    }
    Ok(())
}

fn verify_structural_sourcegraph_select_refusal(socket: &Path) -> TestResult {
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
                cursor: None,
            },
            cursor: None,
        }),
    };
    let mut observed: Option<String> = None;
    let saw_expected = wait_until(READINESS_TIMEOUT, || {
        match send_query_request(socket, &request) {
            Ok(response) => match response.payload {
                SearchPlaneQueryIpcResponse::Error(err)
                    if err.code.as_wire_str() == "NOT_READY"
                        || err.code.as_wire_str() == "STR_GENERATION_NOT_READY" =>
                {
                    observed = Some(err.code.as_wire_str().to_string());
                    false
                }
                SearchPlaneQueryIpcResponse::Error(err) => {
                    observed = Some(format!("{err:?}"));
                    err.code.as_wire_str() == "STR_INVALID_REQUEST"
                        && err.message.contains("filter `select`")
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
        return Err(format!(
            "expected STR_INVALID_REQUEST for structural SG select filter, observed {observed:?}"
        )
        .into());
    }
    Ok(())
}

fn verify_structural_timeout_refusal(socket: &Path) -> TestResult {
    let response = send_query_request(
        socket,
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
                    cursor: None,
                },
                cursor: None,
            }),
        },
    )?;
    let err = match response.payload {
        SearchPlaneQueryIpcResponse::Error(err) => err,
        other => return Err(format!("expected Error, got {other:?}").into()),
    };
    if err.code.as_wire_str() != "STR_INVALID_REQUEST" || !err.message.contains("timeout option") {
        return Err(
            format!("expected STR_INVALID_REQUEST structural timeout error, got {err:?}").into(),
        );
    }
    Ok(())
}

fn verify_structural_typed_holes(socket: &Path) -> TestResult {
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
                cursor: None,
            },
            cursor: None,
        }),
    };
    let mut observed_expr: Option<String> = None;
    let saw_expr = wait_until(READINESS_TIMEOUT, || {
        match send_query_request(socket, &expr_request) {
            Ok(response) => match response.payload {
                SearchPlaneQueryIpcResponse::Structural(structural) => {
                    observed_expr = Some(format!("{structural:?}"));
                    structural.generation == pin && structural.results.len() == 1
                }
                SearchPlaneQueryIpcResponse::Error(err)
                    if err.code.as_wire_str() == "NOT_READY"
                        || err.code.as_wire_str() == "STR_GENERATION_NOT_READY" =>
                {
                    observed_expr = Some(err.code.as_wire_str().to_string());
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
        return Err(format!(
            "typed expr structural query never became ready; observed {observed_expr:?}"
        )
        .into());
    }

    let expr_response = send_query_request(socket, &expr_request)?;
    let expr_structural = match expr_response.payload {
        SearchPlaneQueryIpcResponse::Structural(structural) => structural,
        other => return Err(format!("expected Structural for typed expr, got {other:?}").into()),
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
        return Err(format!("unexpected typed expr candidate/binding: {expr_candidate:?}").into());
    }

    let item_response = send_query_request(
        socket,
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
                    cursor: None,
                },
                cursor: None,
            }),
        },
    )?;
    let item_structural = match item_response.payload {
        SearchPlaneQueryIpcResponse::Structural(structural) => structural,
        other => return Err(format!("expected Structural for typed item, got {other:?}").into()),
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
        return Err(format!("unexpected typed item candidate/binding: {item_candidate:?}").into());
    }
    Ok(())
}

#[test]
fn structural_ready_queries_share_one_indexed_fixture() -> TestResult {
    let fixture = ScenarioFixture::boot()?;
    let socket = fixture.query_socket.clone();
    let ingest_socket = fixture.ingest_socket;
    let mut corpus = SourceCorpusFixture::new(
        repo(),
        revision(),
        generation(),
        format!("fixture-event-{}", next_request_id()),
    );
    publish_structural_ready_fixture(&ingest_socket, &mut corpus)?;
    seal_structural(&ingest_socket)?;

    let verification: TestResult = (|| {
        let verify_structural_sourcegraph_match_fn: fn(&Path) -> TestResult =
            verify_structural_sourcegraph_match;
        for (name, verify) in [
            ("sourcegraph_match", verify_structural_sourcegraph_match_fn),
            (
                "sourcegraph_regex_match",
                verify_structural_sourcegraph_regex_match,
            ),
            (
                "sourcegraph_select_refusal",
                verify_structural_sourcegraph_select_refusal,
            ),
            ("typed_holes", verify_structural_typed_holes),
        ] {
            verify(&socket)
                .map_err(|error| -> Box<dyn Error> { format!("{name}: {error}").into() })?;
        }
        Ok(())
    })();
    verification
}

fn verify_structural_typed_hole_kind_refusal(socket: &Path) -> TestResult {
    let response = send_query_request(
        socket,
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
                    cursor: None,
                },
                cursor: None,
            }),
        },
    )?;
    let err = match response.payload {
        SearchPlaneQueryIpcResponse::Error(err) => err,
        other => return Err(format!("expected Error, got {other:?}").into()),
    };
    if err.code.as_wire_str() != "STR_HOLE_KIND_UNSUPPORTED"
        || !err.message.contains("typed hole kind `lambda`")
    {
        return Err(
            format!("expected STR_HOLE_KIND_UNSUPPORTED typed-hole error, got {err:?}").into(),
        );
    }
    Ok(())
}

#[test]
fn request_validation_refusals_share_one_runtime() -> TestResult {
    let fixture = ScenarioFixture::boot()?;
    let socket = fixture.query_socket;

    let verification: TestResult = (|| {
        let verify_history_generation_not_ready_fn: fn(&Path) -> TestResult =
            verify_history_generation_not_ready;
        for (name, verify) in [
            (
                "history_generation_not_ready",
                verify_history_generation_not_ready_fn,
            ),
            (
                "hybrid_requires_joint_materialization",
                verify_hybrid_requires_joint_materialization,
            ),
            (
                "semantic_requires_materialization",
                verify_semantic_requires_materialization,
            ),
            (
                "hybrid_generation_pin_mismatch",
                verify_hybrid_generation_pin_mismatch,
            ),
            (
                "semantic_generation_pin_mismatch",
                verify_semantic_generation_pin_mismatch,
            ),
            ("hybrid_zero_top_k", verify_hybrid_zero_top_k_refusal),
            (
                "structural_generation_not_ready",
                verify_structural_generation_not_ready,
            ),
            (
                "structural_composition_wiring",
                verify_structural_composition_wiring,
            ),
            (
                "pattern_type_required",
                verify_structural_pattern_type_required,
            ),
            ("timeout_refusal", verify_structural_timeout_refusal),
            (
                "typed_hole_kind_refusal",
                verify_structural_typed_hole_kind_refusal,
            ),
        ] {
            verify(&socket)
                .map_err(|error| -> Box<dyn Error> { format!("{name}: {error}").into() })?;
        }
        Ok(())
    })();
    verification
}

fn lex_query(needle: &str) -> SearchPlaneQueryIpcRequestEnvelope {
    SearchPlaneQueryIpcRequestEnvelope {
        request_id: next_request_id(),
        payload: SearchPlaneQueryIpcRequest::Text(TextQueryRequest {
            syntax: TextQuerySyntax::Sourcegraph,
            query_text: needle.to_string(),
            constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
            generation: Some(GenerationPin::new(repo(), revision(), generation())),
            generation_selector: None,
            top_k: 50,
            cursor: None,
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
        request_id: next_request_id(),
        payload: SearchPlaneQueryIpcRequest::History(HistoryQueryRequest {
            text_query: TextQueryRequest {
                syntax,
                query_text: query_text.to_string(),
                constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                generation: Some(GenerationPin::new(repo(), revision(), generation())),
                generation_selector: None,
                top_k: 50,
                cursor: None,
            },
            order: HistoryOrderV1::Recency,
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
        request_id: next_request_id(),
        payload: SearchPlaneQueryIpcRequest::RuntimeMetadata(RuntimeMetadataQueryRequest {
            text_query: TextQueryRequest {
                syntax,
                query_text: query_text.to_string(),
                constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                generation: Some(GenerationPin::new(repo(), revision(), generation())),
                generation_selector: None,
                top_k: 50,
                cursor: None,
            },
            cursor: None,
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
                last_observed = err.code.as_wire_str().to_string();
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
