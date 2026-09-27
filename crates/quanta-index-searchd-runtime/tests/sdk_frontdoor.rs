//! SDK-frontdoor end-to-end proof for source-authority packets.

#![forbid(unsafe_code)]
#![expect(
    clippy::expect_used,
    reason = "integration-test helpers outside `#[test]` fns assert fixture setup with `expect`; the workspace already permits this inside test fns and a helper that cannot set up its fixture has no caller to propagate to"
)]

use std::cell::Cell;
use std::collections::BTreeSet;
use std::error::Error;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use quanta_index_contract::lex::{
    LanguageCode, LexicalErrorCode, SymbolKindCode, SymbolKindFamily, SymbolRecord,
    SymbolRelationship, SymbolSpan, compute_parse_tree_source_hash,
};
use quanta_index_contract::{
    ChunkId, ChunkRecord, FileContributorIdentityEntry, GenerationPin, GenerationSelector,
    HistoryOrderV1, HistoryQueryRequest, HybridSeedQueryRequest, ManifestGeneration, RepoId,
    RepoMapActivateGenerationRequestV2, RepoMapChunkExactness, RepoMapChunkNode,
    RepoMapContainsEdge, RepoMapDocType, RepoMapEdge, RepoMapExactnessSummary, RepoMapFileNode,
    RepoMapFocusSubjectDto, RepoMapGraphCoverage, RepoMapGraphCoverageClass,
    RepoMapItemIndexAvailability, RepoMapNode, RepoMapNodeRef, RepoMapOwnsChunkEdge,
    RepoMapPublishBundleRequestV2, RepoMapQueryRequest, RepoMapRedactionState, RepoMapSourceBundle,
    RepoMapSymbolNode, RevisionId, RuntimeCatalogIngestBatch, RuntimeChangedRecord,
    RuntimeDocFacetRecord, RuntimeEdgeAuthorityRecord, RuntimeMetadataQueryRequest,
    RuntimeSnapshotRecord, SearchCorpusActiveHeadV1, SearchCorpusGenerationIdentityV1,
    SearchPlaneErrorCodeV2, SearchPlaneIngestIpcRequest, SearchPlaneIngestIpcRequestEnvelope,
    SearchPlaneIngestIpcResponse, SearchPlaneIngestIpcResponseEnvelope, SearchPlaneTrackKind,
    SemanticQueryRequest, SourceFileKey, SourcePublicationEvent, StructuralQueryRequest, SymbolId,
    SymbolQueryRequest, TextQueryRequest, TextQuerySyntax,
};
use quanta_index_ipc::send_request;
use quanta_index_sdk::{
    CommitRecord, CommitSha, ConnectOptions, DiffHunkRecord, DirtyBatch, DirtyRecord,
    FileContributorBatch, FileOwnershipBatch, ParseNode, ParseRoleTag, ParseTreeRecord,
    QuantaIndex, RepoCommitRecencyBatch, RepoDescriptionBatch, RepoMetaBatch, RepoRelativePath,
    RepoTopicBatch, SdkError, SearchCorpusBatch, SearchScopeKey, SearchScopeSurface,
    StructuralBatch,
};
use quanta_index_searchd_harness::{
    E2eRuntime, fixture_source_scope_v1, semantic_source_scopes_for_chunk_records,
};

use crate::fail_closed_wait::{
    RealTicker, UnexpectedSuccess, WaitError, WaitTicker, wait_for, wait_for_terminal_error,
};
use crate::frontdoor_scenarios::{
    SDK_FRONTDOOR_SCENARIOS, SdkFrontdoorExpectation, SdkFrontdoorSurface,
};
use crate::searchd_binary_process::SearchdBinaryProcess;

type TestResult = Result<(), Box<dyn Error>>;

static NEXT_REQUEST_ID: AtomicU64 = AtomicU64::new(1);
const SOCKET_TIMEOUT: Duration = Duration::from_secs(5);

fn next_request_id() -> u64 {
    let id = NEXT_REQUEST_ID.fetch_add(1, Ordering::Relaxed);
    assert_ne!(id, 0, "test request IDs must remain nonzero");
    id
}

/// Harness-owned SDK frontdoor fixture (TOPT-06: the last direct-runtime
///
/// builder joins the TOPT-03 owner; waits over it are fail-closed per
/// TH-1). Boot binds query/control/ingest under the same retention
/// policy the old `build_config` spelled out; explicit `stop` surfaces
/// a driver failure, drop owns unwind cleanup — no manual shutdown tails.
struct SdkFrontdoorRuntime {
    runtime: E2eRuntime,
    client: QuantaIndex,
    ingest_socket: PathBuf,
}

impl SdkFrontdoorRuntime {
    fn start() -> Result<Self, Box<dyn Error>> {
        let mut runtime = E2eRuntime::boot()?;
        runtime.start()?;
        Self::connect(runtime)
    }

    /// Serve a caller-owned root (the multigen restart): the directory
    /// outlives each boot, so stop plus start replays a restart.
    fn start_at(state_root: &Path) -> Result<Self, Box<dyn Error>> {
        let mut runtime = E2eRuntime::boot_in(state_root)?;
        runtime.start()?;
        Self::connect(runtime)
    }

    fn connect(runtime: E2eRuntime) -> Result<Self, Box<dyn Error>> {
        let (query, control, ingest) = runtime
            .socket_paths()
            .ok_or_else(|| "fixture: driver started without socket paths".to_string())
            .map(|(query, control, ingest)| {
                (
                    query.to_path_buf(),
                    control.to_path_buf(),
                    ingest.to_path_buf(),
                )
            })?;
        let client = QuantaIndex::connect(
            ConnectOptions::from_state_root(runtime.state_root())
                .with_query_socket(query)
                .with_control_socket(control)
                .with_ingest_socket(ingest.clone()),
        )?;
        Ok(Self {
            runtime,
            client,
            ingest_socket: ingest,
        })
    }

    fn stop(self) -> TestResult {
        Ok(self.runtime.stop()?)
    }
}

fn repo() -> RepoId {
    RepoId::new("repo-sdk").expect("static fixture ID satisfies canonical policy")
}

fn revision() -> RevisionId {
    RevisionId::new("rev-sdk").expect("static fixture ID satisfies canonical policy")
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

fn pinned_selector(pin: GenerationPin) -> GenerationSelector {
    GenerationSelector::Pinned(pin)
}

fn commit_sha() -> CommitSha {
    CommitSha::from_bytes([
        0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd,
        0xef, 0x01, 0x23, 0x45, 0x67,
    ])
}

fn dispatch_ingest(socket: &Path, payload: SearchPlaneIngestIpcRequest) -> TestResult {
    // Like every producer, stamp the canonical batch digest before sending
    // (QI-BB-032); the search plane refuses any other digest.
    let payload = quanta_index_searchd_harness::stamped_ingest_request(payload)?;
    let response: SearchPlaneIngestIpcResponseEnvelope = send_request(
        socket,
        &SearchPlaneIngestIpcRequestEnvelope {
            request_id: next_request_id(),
            payload,
        },
        quanta_index_ipc::ClientIoPolicy::default(),
    )?;
    match response.payload {
        SearchPlaneIngestIpcResponse::Error(err) => {
            Err(format!("ingest failed code={} message={}", err.code, err.message).into())
        }
        SearchPlaneIngestIpcResponse::SearchCorpusReceipt(_)
        | SearchPlaneIngestIpcResponse::HistoryReceipt(_)
        | SearchPlaneIngestIpcResponse::RepoCommitRecencyReceipt(_)
        | SearchPlaneIngestIpcResponse::RepoTopicReceipt(_)
        | SearchPlaneIngestIpcResponse::FileOwnershipReceipt(_)
        | SearchPlaneIngestIpcResponse::FileContributorReceipt(_)
        | SearchPlaneIngestIpcResponse::DirtyReceipt(_)
        | SearchPlaneIngestIpcResponse::RuntimeCatalogReceipt(_)
        | SearchPlaneIngestIpcResponse::StructuralReceipt(_)
        | SearchPlaneIngestIpcResponse::RepoMapTerminalReceiptV2(_)
        | SearchPlaneIngestIpcResponse::RepoMetaReceipt(_)
        | SearchPlaneIngestIpcResponse::RepoDescriptionReceipt(_) => Ok(()),
    }
}

fn history_batch() -> quanta_index_sdk::HistoryBatch {
    quanta_index_sdk::HistoryBatch::new(repo(), revision(), generation())
        .manifest_digest("manifest:history-sdk")
        .commit(CommitRecord {
            wire_version: 1,
            sha: commit_sha(),
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
    quanta_index_sdk::HistoryBatch::new(repo(), revision(), generation())
        .manifest_digest("manifest:history-sdk-commit-only")
        .commit(CommitRecord {
            wire_version: 1,
            sha: commit_sha(),
            parents: Vec::new(),
            author_time_ms: 21,
            committer_time_ms: 22,
            applied_at_ms: 23,
            author: "alice".to_string().into_boxed_str(),
            author_name: None,
            author_email: None,
            committer: "alice".to_string().into_boxed_str(),
            committer_name: None,
            committer_email: None,
            message: "todo: shard gap".to_string().into_boxed_str(),
            is_merge: false,
            tags: Vec::new(),
        })
}

fn lexical_batch() -> Result<SearchCorpusBatch, Box<dyn Error>> {
    let batch =
        SearchCorpusBatch::replace_generation(repo(), revision(), generation(), "manifest:lexical")
            .source_event(lexical_event("fixture:sdk-lexical-v1", None));
    let batch = replace_fixture_file(
        batch,
        repo(),
        revision(),
        "src/lib.rs",
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
    )?;
    let batch = replace_fixture_file(
        batch,
        repo(),
        revision(),
        "src/alpha.rs",
        vec![lexical_chunk(
            "alpha",
            "src/alpha.rs",
            "sphinx of quartz",
            "text:alpha",
            "shape:alpha",
            16,
        )?],
        Vec::new(),
    )?;
    let batch = replace_fixture_file(
        batch,
        repo(),
        revision(),
        "src/beta.rs",
        vec![lexical_chunk(
            "beta",
            "src/beta.rs",
            "sphinx riddles",
            "text:beta",
            "shape:beta",
            14,
        )?],
        Vec::new(),
    )?;
    Ok(with_semantic_sources_from_chunks(batch))
}

fn lexical_frontdoor_matrix_batch() -> Result<SearchCorpusBatch, Box<dyn Error>> {
    let batch = lexical_batch()?;
    let batch = replace_fixture_file(
        batch,
        repo(),
        revision(),
        "src/file_contains.rs",
        vec![lexical_chunk(
            "chunk-file-contains",
            "src/file_contains.rs",
            "foo oo_ba file_contains_needle",
            "text:file-contains",
            "shape:file-contains",
            29,
        )?],
        Vec::new(),
    )?;
    let batch = replace_fixture_file(
        batch,
        RepoId::new("corp-a")?,
        revision(),
        "src/recency_a.rs",
        vec![lexical_chunk_with_source_repo(
            "chunk-recency-a",
            "src/recency_a.rs",
            "shared_oracle_needle corp-a branch",
            "text:recency-a",
            "shape:recency-a",
            33,
            Some("corp-a"),
        )?],
        Vec::new(),
    )?;
    let batch = replace_fixture_file(
        batch,
        RepoId::new("corp-a")?,
        revision(),
        "src/recency_gate.rs",
        vec![lexical_chunk_with_source_repo(
            "chunk-recency-a-gate",
            "src/recency_gate.rs",
            "shared_oracle_needle gate-a only",
            "text:recency-gate",
            "shape:recency-gate",
            32,
            Some("corp-a"),
        )?],
        Vec::new(),
    )?;
    replace_fixture_file(
        batch,
        RepoId::new("corp-b")?,
        revision(),
        "src/recency_b.rs",
        vec![lexical_chunk_with_source_repo(
            "chunk-recency-b",
            "src/recency_b.rs",
            "shared_oracle_needle corp-b branch",
            "text:recency-b",
            "shape:recency-b",
            33,
            Some("corp-b"),
        )?],
        Vec::new(),
    )
}

fn lexical_batch_two() -> Result<SearchCorpusBatch, Box<dyn Error>> {
    let batch = SearchCorpusBatch::replace_generation(
        repo(),
        revision(),
        generation_two(),
        "manifest:lexical-v2",
    )
    .source_event(lexical_event(
        "fixture:sdk-lexical-v2",
        Some("fixture:sdk-lexical-v1"),
    ));
    let batch = replace_fixture_file(
        batch,
        repo(),
        revision(),
        "src/lib.rs",
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
    )?;
    let batch = replace_fixture_file(
        batch,
        repo(),
        revision(),
        "src/gamma.rs",
        vec![lexical_chunk(
            "gamma",
            "src/gamma.rs",
            "obsidian gamma",
            "text:gamma",
            "shape:gamma",
            14,
        )?],
        Vec::new(),
    )?;
    Ok(with_semantic_sources_from_chunks(batch))
}

fn lexical_event(event_id: &str, expected_base_event_id: Option<&str>) -> SourcePublicationEvent {
    SourcePublicationEvent {
        stream_id: "fixture:sdk-frontdoor".to_string(),
        event_id: event_id.to_string(),
        expected_base_event_id: expected_base_event_id.map(str::to_string),
        payload_sha256: [0; 32], // The SDK stamps the committed payload.
    }
}

fn replace_fixture_file(
    batch: SearchCorpusBatch,
    source_repo_id: RepoId,
    revision_id: RevisionId,
    path: &str,
    chunks: Vec<ChunkRecord>,
    symbols: Vec<SymbolRecord>,
) -> Result<SearchCorpusBatch, Box<dyn Error>> {
    let no_symbols = symbols.is_empty();
    let mut scope = fixture_source_scope_v1(
        SourceFileKey {
            source_repo_id,
            repo_relative_path: RepoRelativePath::new(path),
        },
        revision_id,
        chunks,
        symbols,
    )?;
    if no_symbols {
        // The synthetic fixture has run symbol extraction and found none.
        // A known empty result is Complete(0), not NotRequested.
        scope.coverage.symbols =
            quanta_index_contract::SymbolCoverage::Complete { symbol_count: 0 };
    }
    Ok(batch.replace_scope(scope.coverage, scope.chunks, scope.symbols))
}

fn with_semantic_sources_from_chunks(mut batch: SearchCorpusBatch) -> SearchCorpusBatch {
    let chunks: Vec<_> = batch
        .replace_scopes()
        .iter()
        .flat_map(|scope| scope.chunks.iter().cloned())
        .collect();
    for scope in semantic_source_scopes_for_chunk_records(&chunks) {
        batch = batch.replace_semantic_scope(
            scope.scope,
            scope.scope_digest,
            scope.sources,
            scope.cluster_memberships,
        );
    }
    batch
}

fn lexical_chunk(
    chunk_id: &str,
    path: &str,
    snippet: &str,
    text_digest: &str,
    shape_digest: &str,
    end_byte: u32,
) -> Result<ChunkRecord, Box<dyn Error>> {
    lexical_chunk_with_source_repo(
        chunk_id,
        path,
        snippet,
        text_digest,
        shape_digest,
        end_byte,
        None,
    )
}

fn lexical_chunk_with_source_repo(
    chunk_id: &str,
    path: &str,
    snippet: &str,
    _text_digest: &str,
    _shape_digest: &str,
    end_byte: u32,
    source_repo_id: Option<&str>,
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
        source_repo_id: source_repo_id.map(RepoId::new).transpose()?,
    })
}

fn now_epoch_ms() -> Result<u64, Box<dyn Error>> {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|err| -> Box<dyn Error> {
            format!("system time before unix epoch: {err}").into()
        })?
        .as_millis();
    u64::try_from(millis)
        .map_err(|err| -> Box<dyn Error> { format!("epoch millis overflow u64: {err}").into() })
}

fn repo_commit_recency_batch() -> Result<RepoCommitRecencyBatch, Box<dyn Error>> {
    let now_ms = now_epoch_ms()?;
    Ok(
        RepoCommitRecencyBatch::new(repo(), revision(), generation())
            .entry(
                RepoId::new("corp-a").expect("static fixture ID satisfies canonical policy"),
                now_ms.saturating_sub(6 * 60 * 60 * 1000),
            )
            .entry(
                RepoId::new("corp-b").expect("static fixture ID satisfies canonical policy"),
                1_700_000_000_000,
            ),
    )
}

fn repo_meta_batch() -> RepoMetaBatch {
    RepoMetaBatch::new(repo(), revision(), generation())
        .entry(
            RepoId::new("corp-a").expect("static fixture ID satisfies canonical policy"),
            "license",
            "apache-2.0",
        )
        .entry(
            RepoId::new("corp-b").expect("static fixture ID satisfies canonical policy"),
            "license",
            "gpl-3.0",
        )
}

fn repo_topic_batch() -> RepoTopicBatch {
    RepoTopicBatch::new(repo(), revision(), generation())
        .entry(
            RepoId::new("corp-a").expect("static fixture ID satisfies canonical policy"),
            "security",
        )
        .entry(
            RepoId::new("corp-a").expect("static fixture ID satisfies canonical policy"),
            "platform",
        )
        .entry(
            RepoId::new("corp-b").expect("static fixture ID satisfies canonical policy"),
            "ml",
        )
}

fn repo_description_batch() -> RepoDescriptionBatch {
    RepoDescriptionBatch::new(repo(), revision(), generation())
        .entry(
            RepoId::new("corp-a").expect("static fixture ID satisfies canonical policy"),
            "Apache distributed systems platform",
        )
        .entry(
            RepoId::new("corp-b").expect("static fixture ID satisfies canonical policy"),
            "Machine learning training pipelines",
        )
}

fn file_ownership_batch() -> FileOwnershipBatch {
    FileOwnershipBatch::new(repo(), revision(), generation())
        .entry(
            RepoId::new("corp-a").expect("static fixture ID satisfies canonical policy"),
            RepoRelativePath::new("src/recency_a.rs"),
            vec!["@alice".to_string(), "@acme/platform".to_string()],
        )
        .entry(
            RepoId::new("corp-a").expect("static fixture ID satisfies canonical policy"),
            RepoRelativePath::new("src/recency_gate.rs"),
            vec!["@alice".to_string()],
        )
        .entry(
            RepoId::new("corp-b").expect("static fixture ID satisfies canonical policy"),
            RepoRelativePath::new("src/recency_b.rs"),
            vec!["@bob".to_string()],
        )
}

fn file_contributor_batch() -> FileContributorBatch {
    FileContributorBatch::new(repo(), revision(), generation())
        .entry_identities(
            RepoId::new("corp-a").expect("static fixture ID satisfies canonical policy"),
            RepoRelativePath::new("src/recency_a.rs"),
            vec![
                FileContributorIdentityEntry {
                    canonical: "alice".to_string(),
                    name: Some("Alice Example".to_string()),
                    email: Some("alice@example.com".to_string()),
                },
                FileContributorIdentityEntry {
                    canonical: "carol".to_string(),
                    name: Some("Carol Example".to_string()),
                    email: Some("carol@example.com".to_string()),
                },
            ],
        )
        .entry_identities(
            RepoId::new("corp-a").expect("static fixture ID satisfies canonical policy"),
            RepoRelativePath::new("src/recency_gate.rs"),
            vec![FileContributorIdentityEntry {
                canonical: "alice".to_string(),
                name: Some("Alice Example".to_string()),
                email: Some("alice@example.com".to_string()),
            }],
        )
        .entry_identities(
            RepoId::new("corp-b").expect("static fixture ID satisfies canonical policy"),
            RepoRelativePath::new("src/recency_b.rs"),
            vec![FileContributorIdentityEntry {
                canonical: "bob".to_string(),
                name: Some("Bob Builder".to_string()),
                email: Some("bob@example.com".to_string()),
            }],
        )
}

fn rev_at_time_ancestor_revision() -> RevisionId {
    RevisionId::new("1111111111111111111111111111111111111111")
        .expect("static fixture ID satisfies canonical policy")
}

fn rev_at_time_head_revision() -> RevisionId {
    RevisionId::new("2222222222222222222222222222222222222222")
        .expect("static fixture ID satisfies canonical policy")
}

fn rev_at_time_ancestor_generation() -> ManifestGeneration {
    ManifestGeneration::new(41)
}

fn rev_at_time_head_generation() -> ManifestGeneration {
    ManifestGeneration::new(42)
}

fn rev_at_time_ancestor_pin() -> GenerationPin {
    GenerationPin::new(
        repo(),
        rev_at_time_ancestor_revision(),
        rev_at_time_ancestor_generation(),
    )
}

fn rev_at_time_head_pin() -> GenerationPin {
    GenerationPin::new(
        repo(),
        rev_at_time_head_revision(),
        rev_at_time_head_generation(),
    )
}

fn rev_at_time_lexical_batch(
    revision_id: RevisionId,
    generation: ManifestGeneration,
    event_id: &str,
    expected_base_event_id: Option<&str>,
    path: &str,
    candidate_id: &str,
    snippet: &str,
) -> Result<SearchCorpusBatch, Box<dyn Error>> {
    let batch = SearchCorpusBatch::replace_generation(
        repo(),
        revision_id.clone(),
        generation,
        format!("manifest:rev-at-time:{}:{}", path, generation.get()),
    )
    .source_event(lexical_event(event_id, expected_base_event_id));
    replace_fixture_file(
        batch,
        repo(),
        revision_id,
        path,
        vec![lexical_chunk(
            candidate_id,
            path,
            snippet,
            "text:rev-at-time",
            "shape:rev-at-time",
            u32::try_from(snippet.len()).map_err(|err| -> Box<dyn Error> {
                format!("rev_at_time lexical chunk overflow: {err}").into()
            })?,
        )?],
        Vec::new(),
    )
}

fn rev_at_time_history_batch() -> Result<quanta_index_sdk::HistoryBatch, Box<dyn Error>> {
    let now_ms = now_epoch_ms()?;
    Ok(quanta_index_sdk::HistoryBatch::new(
        repo(),
        rev_at_time_head_revision(),
        rev_at_time_head_generation(),
    )
    .manifest_digest("manifest:rev-at-time-history-sdk")
    .commit(CommitRecord {
        wire_version: 1,
        sha: CommitSha::from_hex("1111111111111111111111111111111111111111").map_err(
            |err| -> Box<dyn Error> { format!("invalid rev-at-time ancestor sha: {err}").into() },
        )?,
        parents: Vec::new(),
        author_time_ms: now_ms.saturating_sub(63_072_000_000),
        committer_time_ms: now_ms.saturating_sub(63_072_000_000),
        applied_at_ms: now_ms.saturating_sub(63_072_000_000),
        author: "alice".to_string().into_boxed_str(),
        author_name: None,
        author_email: None,
        committer: "alice".to_string().into_boxed_str(),
        committer_name: None,
        committer_email: None,
        message: "legacy rev-at-time commit".to_string().into_boxed_str(),
        is_merge: false,
        tags: Vec::new(),
    })
    .commit(CommitRecord {
        wire_version: 1,
        sha: CommitSha::from_hex("2222222222222222222222222222222222222222").map_err(
            |err| -> Box<dyn Error> { format!("invalid rev-at-time head sha: {err}").into() },
        )?,
        parents: vec![
            CommitSha::from_hex("1111111111111111111111111111111111111111").map_err(
                |err| -> Box<dyn Error> { format!("invalid rev-at-time parent sha: {err}").into() },
            )?,
        ],
        author_time_ms: now_ms.saturating_sub(12 * 60 * 60 * 1000),
        committer_time_ms: now_ms.saturating_sub(12 * 60 * 60 * 1000),
        applied_at_ms: now_ms.saturating_sub(12 * 60 * 60 * 1000),
        author: "alice".to_string().into_boxed_str(),
        author_name: None,
        author_email: None,
        committer: "alice".to_string().into_boxed_str(),
        committer_name: None,
        committer_email: None,
        message: "head rev-at-time commit".to_string().into_boxed_str(),
        is_merge: false,
        tags: Vec::new(),
    })
    .ref_upsert(
        "HEAD",
        CommitSha::from_hex("2222222222222222222222222222222222222222").map_err(
            |err| -> Box<dyn Error> { format!("invalid rev-at-time HEAD sha: {err}").into() },
        )?,
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
    DirtyBatch::new(repo(), revision(), generation(), 1_717_171_717_000)
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
        "2".repeat(64),
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
        symbol_kind: symbol_kind.clone(),
    }))
    .with_node(RepoMapNode::Symbol(RepoMapSymbolNode {
        symbol_id: SymbolId::new("symbol://gamma"),
        owner_path: RepoRelativePath::new("tests/repo_map.rs"),
        local_name: "Gamma".to_string(),
        qualified_name: "tests::repo_map::Gamma".to_string(),
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
        callee: RepoMapNodeRef::Symbol(SymbolId::new("symbol://gamma")),
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

fn repo_map_activate_request() -> Result<RepoMapActivateGenerationRequestV2, Box<dyn Error>> {
    Ok(RepoMapActivateGenerationRequestV2::for_bundle(
        &repo_map_bundle()?,
    )?)
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
    Ok(
        StructuralBatch::replace_generation(
            repo(),
            revision(),
            generation(),
            "manifest:structural",
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
        ),
    )
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
        SdkError::Remote { code, .. } if code.as_wire_str() == expected => Ok(()),
        other @ (SdkError::Usage(_)
        | SdkError::Protocol(_)
        | SdkError::Serialization(_)
        | SdkError::Transport(_)
        | SdkError::Remote { .. }
        | SdkError::Binding { .. }
        | SdkError::PlaneUnavailable { .. }) => {
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

fn current_sdk_search_corpus_or_none(
    client: &QuantaIndex,
    repo_id: RepoId,
    revision_id: RevisionId,
) -> Result<Option<SearchCorpusActiveHeadV1>, SdkError> {
    client.generations().active_head(repo_id, revision_id)
}

/// Publishes once and promotes one complete corpus identity.
///
/// This deliberately does not use the polling helper: retrying a mutating
/// publish after an ambiguous transport or CAS outcome could duplicate the
/// ingress operation.
fn publish_and_activate_sdk_search_corpus(
    client: &QuantaIndex,
    batch: &SearchCorpusBatch,
) -> Result<SearchCorpusGenerationIdentityV1, Box<dyn Error>> {
    let expected_active = current_sdk_search_corpus_or_none(
        client,
        batch.repo_id().clone(),
        batch.revision_id().clone(),
    )?;
    let ack = client
        .search_corpus()
        .publish_and_activate(batch, expected_active)?;
    Ok(ack.1.active.generation)
}

fn publish_sdk_search_corpus_ready(client: &QuantaIndex) -> TestResult {
    let batch = lexical_batch()?;
    let _active = publish_and_activate_sdk_search_corpus(client, &batch)?;
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

/// Poll cadence shared by the SDK frontdoor waits: the TH-1 repair
/// keeps the historical 10ms cadence and changes only the timeout
/// contract underneath it.
const SDK_WAIT_POLL: Duration = Duration::from_millis(10);

/// Timeout evidence names the predicate class and the exact wait call
/// site, so a spent wait points at the observation that never arrived.
fn wait_description(what: &str, caller: &std::panic::Location<'_>) -> String {
    format!(
        "{what} (wait called at {}:{})",
        caller.file(),
        caller.line()
    )
}

#[track_caller]
fn wait_for_sdk_ready<T, F>(timeout: Duration, run: F) -> Result<T, WaitError<SdkError>>
where
    T: std::fmt::Debug,
    F: FnMut() -> Result<T, SdkError>,
{
    wait_for_sdk_ready_with_ticker(&RealTicker::new(), timeout, run)
}

#[track_caller]
fn wait_for_sdk_ready_with_ticker<T, F>(
    ticker: &dyn WaitTicker,
    timeout: Duration,
    run: F,
) -> Result<T, WaitError<SdkError>>
where
    T: std::fmt::Debug,
    F: FnMut() -> Result<T, SdkError>,
{
    wait_for(
        ticker,
        timeout,
        SDK_WAIT_POLL,
        &wait_description(
            "sdk ready (first non-NOT_READY response)",
            std::panic::Location::caller(),
        ),
        run,
        |_| true,
        |error| matches!(error, SdkError::Remote { code, .. } if code.as_wire_str() == "NOT_READY"),
    )
}

#[track_caller]
fn wait_for_sdk_observation<T, F, P>(
    timeout: Duration,
    run: F,
    ready: P,
) -> Result<T, WaitError<SdkError>>
where
    T: std::fmt::Debug,
    F: FnMut() -> Result<T, SdkError>,
    P: FnMut(&T) -> bool,
{
    wait_for_sdk_observation_at(
        std::panic::Location::caller(),
        timeout,
        &["NOT_READY"],
        run,
        ready,
    )
}

#[track_caller]
fn wait_for_sdk_observation_with_retry_codes<T, F, P>(
    timeout: Duration,
    retry_codes: &[&str],
    run: F,
    ready: P,
) -> Result<T, WaitError<SdkError>>
where
    T: std::fmt::Debug,
    F: FnMut() -> Result<T, SdkError>,
    P: FnMut(&T) -> bool,
{
    wait_for_sdk_observation_at(
        std::panic::Location::caller(),
        timeout,
        retry_codes,
        run,
        ready,
    )
}

fn wait_for_sdk_observation_at<T, F, P>(
    caller: &std::panic::Location<'_>,
    timeout: Duration,
    retry_codes: &[&str],
    run: F,
    ready: P,
) -> Result<T, WaitError<SdkError>>
where
    T: std::fmt::Debug,
    F: FnMut() -> Result<T, SdkError>,
    P: FnMut(&T) -> bool,
{
    wait_for(
        &RealTicker::new(),
        timeout,
        SDK_WAIT_POLL,
        &wait_description("sdk observation satisfying ready", caller),
        run,
        ready,
        |error| matches!(error, SdkError::Remote { code, .. } if retry_codes.contains(&code.as_wire_str())),
    )
}

#[track_caller]
fn wait_for_sdk_terminal_error<T, F>(
    timeout: Duration,
    retry_codes: &[&str],
    run: F,
) -> Result<SdkError, WaitError<UnexpectedSuccess<T>>>
where
    T: std::fmt::Debug,
    F: FnMut() -> Result<T, SdkError>,
{
    wait_for_terminal_error(
        &RealTicker::new(),
        timeout,
        SDK_WAIT_POLL,
        &wait_description(
            "terminal sdk error (no retryable code, no success)",
            std::panic::Location::caller(),
        ),
        run,
        |error| matches!(error, SdkError::Remote { code, .. } if retry_codes.contains(&code.as_wire_str())),
    )
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
        || entry.projection_evidence_kind != "CompiledRepoMapCandidateV1"
    {
        return Err(format!("unexpected repo-map entry: {entry:?}").into());
    }
    // Only included rows come back (QI-BB-008); the rest is a count.
    if response.entries.len() != 1 || response.dropped_entries_count == 0 {
        return Err(format!("unexpected repo-map inclusion set: {response:?}").into());
    }
    // The legacy per-chunk token hint (and the budget-floor degradation it
    // could trigger) was replaced by P02A's compiled envelope: the entry
    // records the compiled-projection evidence and the page reports its
    // drops explicitly instead of inferring them from a hint. Assert the
    // honest drop accounting rather than the removed heuristic.
    if !response
        .drop_reason_codes
        .iter()
        .any(|code| code == "top_k_exhausted")
    {
        return Err(format!("missing repo-map drop reason: {response:?}").into());
    }
    if response
        .degraded_reason_codes
        .iter()
        .any(|code| code == "token_budget_floor_applied" || code == "focus_subjects_unresolved")
    {
        return Err(format!(
            "a strict compiled projection must not report retired degraded reasons: {response:?}"
        )
        .into());
    }
    Ok(())
}

#[track_caller]
fn wait_for_symbol_query<F>(
    timeout: Duration,
    run: F,
) -> Result<quanta_index_contract::SymbolQueryResponse, WaitError<SdkError>>
where
    F: FnMut() -> Result<quanta_index_contract::SymbolQueryResponse, SdkError>,
{
    wait_for_sdk_observation_at(
        std::panic::Location::caller(),
        timeout,
        &["NOT_READY"],
        run,
        |response| response.generation == pin() && response.results.len() == 1,
    )
}

#[test]
fn sdk_publish_frontdoor_routes_ingest_batches() -> TestResult {
    let fixture = SdkFrontdoorRuntime::start()?;
    let client = &fixture.client;

    let lexical_receipt = client.search_corpus().publish(&lexical_batch()?)?;
    let history_receipt = client.history().publish(&history_batch())?;
    let dirty_receipt = client.runtime().publish_dirty(&dirty_batch())?;
    let structural_receipt = client.structural().publish(&structural_batch()?)?;
    let repo_map_receipt = client
        .repomap()
        .publish(&RepoMapPublishBundleRequestV2::new(repo_map_bundle()?)?)?;

    if lexical_receipt.generation != generation()
        || lexical_receipt.accepted_replace_scopes != 3
        || lexical_receipt.accepted_tombstone_scopes != 0
    {
        return Err(format!("unexpected search-corpus receipt: {lexical_receipt:?}").into());
    }
    if history_receipt.generation != generation()
        || history_receipt.accepted_replace_scopes != 4
        || history_receipt.accepted_tombstone_scopes != 0
    {
        return Err(format!("unexpected history receipt: {history_receipt:?}").into());
    }
    if dirty_receipt.generation != generation()
        || dirty_receipt.accepted_replace_scopes != 1
        || dirty_receipt.accepted_tombstone_scopes != 1
    {
        return Err(format!("unexpected dirty receipt: {dirty_receipt:?}").into());
    }
    if structural_receipt.generation != generation()
        || structural_receipt.accepted_replace_scopes != 1
        || structural_receipt.accepted_tombstone_scopes != 0
    {
        return Err(format!("unexpected structural receipt: {structural_receipt:?}").into());
    }
    if repo_map_receipt.mutation.repo_id != repo()
        || repo_map_receipt.mutation.revision_id != revision()
        || repo_map_receipt.mutation.manifest_generation != generation()
    {
        return Err(format!("unexpected repo-map receipt: {repo_map_receipt:?}").into());
    }

    fixture.stop()
}

#[test]
fn sdk_repomap_active_head_tracks_only_catalog_activation() -> TestResult {
    let fixture = SdkFrontdoorRuntime::start()?;
    let client = &fixture.client;

    if client.repomap().active_head(repo(), revision())?.is_some() {
        return Err("fresh RepoMap catalog unexpectedly has an active head".into());
    }
    let publish = client
        .repomap()
        .publish(&RepoMapPublishBundleRequestV2::new(repo_map_bundle()?)?)?;
    if client.repomap().active_head(repo(), revision())?.is_some() {
        return Err("RepoMap publish changed the active head".into());
    }
    let activation = client.repomap().activate(repo_map_activate_request()?)?;
    let head = client
        .repomap()
        .active_head(repo(), revision())?
        .ok_or("RepoMap activation has no catalog head")?;
    if head.epoch().get() != activation.mutation.activation_epoch
        || head.candidate_commitment().to_wire_string()
            != activation.mutation.new_candidate_commitment
        || activation.mutation.new_candidate_commitment != publish.mutation.new_candidate_commitment
    {
        return Err(format!("RepoMap catalog head diverges from receipts: {head:?}").into());
    }

    fixture.stop()
}

#[test]
fn sdk_search_frontdoor_routes_lexical_semantic_hybrid_explain_and_repomap_truth() -> TestResult {
    let fixture = SdkFrontdoorRuntime::start()?;
    let client = &fixture.client;

    let corpus_batch = lexical_batch()?;
    let _corpus_active = publish_and_activate_sdk_search_corpus(client, &corpus_batch)?;
    let repo_map_receipt = client
        .repomap()
        .publish(&RepoMapPublishBundleRequestV2::new(repo_map_bundle()?)?)?;
    if repo_map_receipt.mutation.manifest_generation != generation() {
        return Err(format!("unexpected repo-map publish ack: {repo_map_receipt:?}").into());
    }

    if client.repomap().active_head(repo(), revision())?.is_some() {
        return Err("RepoMap publish activated a head without an activation request".into());
    }

    let repo_map_activation = client.repomap().activate(repo_map_activate_request()?)?;
    if repo_map_activation.mutation.manifest_generation != generation() {
        return Err(format!("unexpected repo-map activate ack: {repo_map_activation:?}").into());
    }
    let active = client
        .repomap()
        .active_head(repo(), revision())?
        .ok_or("RepoMap activation did not expose a catalog head")?;
    if active.epoch().get() != repo_map_activation.mutation.activation_epoch
        || active.candidate_commitment().to_wire_string()
            != repo_map_activation.mutation.new_candidate_commitment
    {
        return Err(format!(
            "RepoMap active head does not match the activation receipt: {active:?}"
        )
        .into());
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
        return Err(format!("unexpected semantic response: {semantic:?}").into());
    }

    let hybrid = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .search()
                .hybrid_seed()
                .sourcegraph("sphinx")
                .semantic_text("quartz")
                .active(repo(), revision())
                .top_k(2)
                .execute()
        },
        |response| response.generation == pin() && !response.seed_candidates.is_empty(),
    )?;
    let hybrid_top = hybrid
        .seed_candidates
        .first()
        .ok_or_else(|| "missing hybrid seed candidate".to_string())?;
    if hybrid.generation != pin()
        || hybrid_top.entity_id != "alpha"
        || hybrid.explanation.summary.is_empty()
    {
        return Err(format!("unexpected hybrid-seed response: {hybrid:?}").into());
    }

    // QI-BB-018: the true-hybrid route from the SDK builder — two
    // independent lanes fused by RRF, every row carrying its lane
    // provenance — and QI-BB-022: its top row explained through the SDK
    // under both queries, re-derived against the index on every axis.
    let true_hybrid = wait_for_sdk_observation(
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
    let true_hybrid_top = true_hybrid
        .results
        .first()
        .ok_or_else(|| "missing hybrid candidate".to_string())?;
    let true_hybrid_rrf: f64 = true_hybrid_top
        .contributions
        .iter()
        .map(|contribution| 1.0 / (60.0 + f64::from(contribution.rank)))
        .sum();
    if true_hybrid.generation != pin()
        || true_hybrid_top.candidate.candidate_id != "alpha"
        || true_hybrid_top.contributions.is_empty()
        || true_hybrid_top.fused_score.to_bits() != true_hybrid_rrf.to_bits()
        || true_hybrid.explanation.summary.is_empty()
    {
        return Err(format!("unexpected hybrid response: {true_hybrid:?}").into());
    }
    let hybrid_explain = wait_for_sdk_ready(SOCKET_TIMEOUT, || {
        client.search().explain_hybrid_under_queries(
            pin(),
            true_hybrid_top.clone(),
            TextQueryRequest {
                syntax: TextQuerySyntax::Sourcegraph,
                query_text: "sphinx".to_string(),
                constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                generation: None,
                generation_selector: None,
                top_k: 2,
                cursor: None,
            },
            "quartz",
        )
    })?;
    let reconciled = |axis: &str| {
        hybrid_explain
            .explanation
            .planner_trace
            .iter()
            .any(|entry| entry.detail == format!("explain.{axis}_reconciled=true"))
    };
    if hybrid_explain.generation != pin()
        || hybrid_explain.explanation.strategy != "hybrid_score_trace"
        || !reconciled("score")
        || !reconciled("dense")
        || !reconciled("fused")
    {
        return Err(format!("unexpected hybrid explain response: {hybrid_explain:?}").into());
    }

    let repo_map = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || client.repomap().query(repo_map_query_request()),
        |response| response.manifest_generation == generation() && !response.entries.is_empty(),
    )?;
    assert_repo_map_happy_path(&repo_map)?;

    fixture.stop()
}

#[test]
fn sdk_query_frontdoor_routes_history_runtime_and_structural_truth() -> TestResult {
    let fixture = SdkFrontdoorRuntime::start()?;
    let client = &fixture.client;

    let corpus_batch = lexical_batch()?;
    let _corpus_active = publish_and_activate_sdk_search_corpus(client, &corpus_batch)?;
    let _history_receipt = client.history().publish(&history_batch())?;
    let _dirty_receipt = client.runtime().publish_dirty(&dirty_batch())?;
    let _structural_receipt = client.structural().publish(&structural_batch()?)?;

    let history_commit = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .history()
                .query()
                .sourcegraph("type:commit rev:refs/heads/main author:alice fix")
                .pinned(pin())
                .top_k(5)
                .order(HistoryOrderV1::Recency)
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
        return Err(format!("unexpected history commit response: {history_commit:?}").into());
    }
    let commit = history_commit
        .commits
        .first()
        .ok_or_else(|| "missing history commit candidate".to_string())?;
    if commit.author != "alice" || commit.message != "fix: sample" {
        return Err(format!("unexpected history commit candidate: {commit:?}").into());
    }

    let history_diff = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .history()
                .query()
                .native("type:diff todo")
                .pinned(pin())
                .top_k(5)
                .order(HistoryOrderV1::Recency)
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
        return Err(format!("unexpected history diff response: {history_diff:?}").into());
    }

    let symbol_select_native = wait_for_symbol_query(SOCKET_TIMEOUT, || {
        client
            .symbol()
            .query()
            .native("select:symbol MySdkSymbol")
            .pinned(pin())
            .top_k(3)
            .execute()
    })?;
    assert_single_symbol_candidate(&symbol_select_native)?;

    let symbol_type_native = wait_for_symbol_query(SOCKET_TIMEOUT, || {
        client
            .symbol()
            .query()
            .native("type:symbol MySdkSymbol")
            .pinned(pin())
            .top_k(3)
            .execute()
    })?;
    assert_single_symbol_candidate(&symbol_type_native)?;

    let symbol_select_sourcegraph = wait_for_symbol_query(SOCKET_TIMEOUT, || {
        client
            .symbol()
            .query()
            .sourcegraph("select:symbol MySdkSymbol")
            .pinned(pin())
            .top_k(3)
            .execute()
    })?;
    assert_single_symbol_candidate(&symbol_select_sourcegraph)?;

    let symbol_type_sourcegraph = wait_for_symbol_query(SOCKET_TIMEOUT, || {
        client
            .symbol()
            .query()
            .sourcegraph("type:symbol MySdkSymbol")
            .pinned(pin())
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
                .pinned(pin())
                .top_k(3)
                .execute()
        },
        |response| response.generation == pin() && response.results.len() == 1,
    )?;
    if runtime_query.generation != pin() || runtime_query.results.len() != 1 {
        return Err(format!("unexpected runtime response: {runtime_query:?}").into());
    }
    let runtime_candidate = runtime_query
        .results
        .first()
        .ok_or_else(|| "missing runtime candidate".to_string())?;
    if runtime_candidate.candidate_id != "chunk-dirty"
        || runtime_candidate.repo_relative_path.as_str() != "src/lib.rs"
    {
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
        return Err(format!("unexpected structural response: {structural_query:?}").into());
    }
    let structural_candidate = structural_query
        .results
        .first()
        .ok_or_else(|| "missing structural candidate".to_string())?;
    if structural_candidate.candidate_id != "chunk-tree" || structural_candidate.bindings.len() != 1
    {
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
        return Err(format!(
            "unexpected pinned structural native candidate: \
             {structural_native_pinned_candidate:?}"
        )
        .into());
    }

    fixture.stop()
}

#[test]
fn sdk_frontdoor_widened_query_matrix_executes_exact_surface_truth() -> TestResult {
    let fixture = SdkFrontdoorRuntime::start()?;
    let client = &fixture.client;
    let ingest_socket = &fixture.ingest_socket;

    // The repo-metadata overlays belong to the generation's sealed
    // contract, so they are published before the seal (QI-BB-030); a
    // publish into the sealed generation would be refused typed.
    let _repo_commit_recency_receipt = client
        .history()
        .publish_repo_commit_recency(&repo_commit_recency_batch()?)?;
    let _repo_meta_receipt = client.history().publish_repo_meta(&repo_meta_batch())?;
    let _repo_description_receipt = client
        .history()
        .publish_repo_description(&repo_description_batch())?;
    let _repo_topic_receipt = client.history().publish_repo_topic(&repo_topic_batch())?;
    let _file_ownership_receipt = client
        .history()
        .publish_file_ownership(&file_ownership_batch())?;
    let _file_contributor_receipt = client
        .history()
        .publish_file_contributor(&file_contributor_batch())?;
    let corpus_batch = lexical_frontdoor_matrix_batch()?;
    let _corpus_active = publish_and_activate_sdk_search_corpus(client, &corpus_batch)?;
    let _history_receipt = client.history().publish(&history_batch())?;
    let _structural_receipt = client.structural().publish(&structural_batch()?)?;
    let _dirty_receipt = client.runtime().publish_dirty(&dirty_batch())?;
    publish_runtime_catalog_batch(ingest_socket)?;

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
                                .pinned(pin())
                                .top_k(10)
                                .execute(),
                            TextQuerySyntax::Sourcegraph => client
                                .lexical()
                                .query()
                                .sourcegraph(scenario.query_text)
                                .pinned(pin())
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
                        return Err(format!(
                            "{} symbol candidate drift: expected {:?}, got {:?}",
                            scenario.name, expected, observed
                        )
                        .into());
                    }
                }
                SdkFrontdoorSurface::Structural => {
                    let response = wait_for_sdk_observation(
                        SOCKET_TIMEOUT,
                        || match scenario.syntax {
                            TextQuerySyntax::Native => client
                                .structural()
                                .query()
                                .native(scenario.query_text)
                                .pinned(pin())
                                .top_k(10)
                                .execute(),
                            TextQuerySyntax::Sourcegraph => client
                                .structural()
                                .query()
                                .sourcegraph(scenario.query_text)
                                .pinned(pin())
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
                        return Err(format!(
                            "{} structural candidate drift: expected {:?}, got {:?}",
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
                                .pinned(pin())
                                .top_k(10)
                                .execute(),
                            TextQuerySyntax::Sourcegraph => client
                                .runtime()
                                .query()
                                .sourcegraph(scenario.query_text)
                                .pinned(pin())
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
                        return Err(format!(
                            "{} runtime candidate drift: expected {:?}, got {:?}",
                            scenario.name, expected, observed
                        )
                        .into());
                    }
                }
                SdkFrontdoorSurface::History => {
                    return Err(format!(
                        "{} used candidate-id expectation on history surface",
                        scenario.name
                    )
                    .into());
                }
            },
            SdkFrontdoorExpectation::CommitShas(expected_shas) => {
                if scenario.surface != SdkFrontdoorSurface::History {
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
                            .pinned(pin())
                            .top_k(10)
                            .order(HistoryOrderV1::Recency)
                            .execute(),
                        TextQuerySyntax::Sourcegraph => client
                            .history()
                            .query()
                            .sourcegraph(scenario.query_text)
                            .pinned(pin())
                            .top_k(10)
                            .order(HistoryOrderV1::Recency)
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
                    return Err(format!(
                        "{} history commit drift: expected {:?}, got commits={:?} diffs={:?}",
                        scenario.name, expected, observed, response.diffs
                    )
                    .into());
                }
            }
            SdkFrontdoorExpectation::TypedError(expected_error) => {
                let err = match scenario.surface {
                    SdkFrontdoorSurface::Lexical => expect_sdk_error(
                        match scenario.syntax {
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
                        &format!("{} lexical typed error", scenario.name),
                    )?,
                    SdkFrontdoorSurface::History => {
                        wait_for_sdk_terminal_error(SOCKET_TIMEOUT, &["NOT_READY"], || {
                            match scenario.syntax {
                                TextQuerySyntax::Native => client
                                    .history()
                                    .query()
                                    .native(scenario.query_text)
                                    .pinned(pin())
                                    .top_k(10)
                                    .order(HistoryOrderV1::Recency)
                                    .execute(),
                                TextQuerySyntax::Sourcegraph => client
                                    .history()
                                    .query()
                                    .sourcegraph(scenario.query_text)
                                    .pinned(pin())
                                    .top_k(10)
                                    .order(HistoryOrderV1::Recency)
                                    .execute(),
                            }
                        })?
                    }
                    SdkFrontdoorSurface::Structural => expect_sdk_error(
                        match scenario.syntax {
                            TextQuerySyntax::Native => client
                                .structural()
                                .query()
                                .native(scenario.query_text)
                                .pinned(pin())
                                .top_k(10)
                                .execute(),
                            TextQuerySyntax::Sourcegraph => client
                                .structural()
                                .query()
                                .sourcegraph(scenario.query_text)
                                .pinned(pin())
                                .top_k(10)
                                .execute(),
                        },
                        &format!("{} structural typed error", scenario.name),
                    )?,
                    other
                    @ (SdkFrontdoorSurface::Symbol | SdkFrontdoorSurface::RuntimeMetadata) => {
                        return Err(format!(
                            "{} typed error expectation on unsupported SDK surface {:?}",
                            scenario.name, other
                        )
                        .into());
                    }
                };
                match err {
                    SdkError::Remote { code, message, .. }
                        if code.as_wire_str() == expected_error.code
                            && message.contains(expected_error.message_contains) => {}
                    other @ (SdkError::Usage(_)
                    | SdkError::Protocol(_)
                    | SdkError::Serialization(_)
                    | SdkError::Transport(_)
                    | SdkError::Remote { .. }
                    | SdkError::Binding { .. }
                    | SdkError::PlaneUnavailable { .. }) => {
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

    fixture.stop()
}

#[test]
fn sdk_text_frontdoor_rebinds_rev_at_time_generation_truth() -> TestResult {
    let fixture = SdkFrontdoorRuntime::start()?;
    let client = &fixture.client;

    let ancestor_batch = rev_at_time_lexical_batch(
        rev_at_time_ancestor_revision(),
        rev_at_time_ancestor_generation(),
        "fixture:rev-at-time:ancestor",
        None,
        "src/legacy.rs",
        "chunk-rev-at-time-ancestor",
        "needle_token legacy_choice",
    )?;
    let _ancestor_active = publish_and_activate_sdk_search_corpus(client, &ancestor_batch)?;
    let head_batch = rev_at_time_lexical_batch(
        rev_at_time_head_revision(),
        rev_at_time_head_generation(),
        "fixture:rev-at-time:head",
        Some("fixture:rev-at-time:ancestor"),
        "src/head.rs",
        "chunk-rev-at-time-head",
        "needle_token head_choice",
    )?;
    let _head_active = publish_and_activate_sdk_search_corpus(client, &head_batch)?;
    let _history_receipt = client.history().publish(&rev_at_time_history_batch()?)?;

    let head = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .lexical()
                .query()
                .sourcegraph("rev:at.time(2100-01-01T00:00:00Z) needle_token")
                .pinned(rev_at_time_head_pin())
                .top_k(10)
                .execute()
        },
        |response| response.generation == rev_at_time_head_pin() && response.results.len() == 1,
    )?;
    let [head_candidate] = head.results.as_slice() else {
        return Err(format!("unexpected future rev:at.time response: {head:?}").into());
    };
    if head_candidate.candidate_id != "chunk-rev-at-time-head" {
        return Err(format!("unexpected future rev:at.time response: {head:?}").into());
    }

    let relative = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .lexical()
                .query()
                .sourcegraph("rev:at.time(1 year ago) needle_token")
                .pinned(rev_at_time_head_pin())
                .top_k(10)
                .execute()
        },
        |response| response.generation == rev_at_time_ancestor_pin() && response.results.len() == 1,
    )?;
    let [relative_candidate] = relative.results.as_slice() else {
        return Err(format!("unexpected human relative rev:at.time response: {relative:?}").into());
    };
    if relative_candidate.candidate_id != "chunk-rev-at-time-ancestor" {
        return Err(format!("unexpected human relative rev:at.time response: {relative:?}").into());
    }

    let named = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .lexical()
                .query()
                .sourcegraph("rev:at.time(yesterday) needle_token")
                .pinned(rev_at_time_head_pin())
                .top_k(10)
                .execute()
        },
        |response| response.generation == rev_at_time_ancestor_pin() && response.results.len() == 1,
    )?;
    let [named_candidate] = named.results.as_slice() else {
        return Err(format!("unexpected named relative rev:at.time response: {named:?}").into());
    };
    if named_candidate.candidate_id != "chunk-rev-at-time-ancestor" {
        return Err(format!("unexpected named relative rev:at.time response: {named:?}").into());
    }

    let calendar = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .lexical()
                .query()
                .sourcegraph("rev:at.time(june 25 2017) needle_token")
                .pinned(rev_at_time_head_pin())
                .top_k(10)
                .execute()
        },
        |response| response.generation == rev_at_time_head_pin() && response.results.is_empty(),
    )?;
    if !calendar.results.is_empty() {
        return Err(format!("unexpected calendar rev:at.time response: {calendar:?}").into());
    }

    let invalid = expect_sdk_error(
        client
            .lexical()
            .query()
            .sourcegraph("rev:at.time(definitely-not-a-timeref) needle_token")
            .pinned(rev_at_time_head_pin())
            .top_k(10)
            .execute(),
        "rev:at.time invalid timeref",
    )?;
    expect_remote_code(invalid, "HISTORY_INVALID_TIMEREF")?;

    fixture.stop()
}

#[test]
fn sdk_history_query_frontdoor_surfaces_typed_absent_and_shard_errors() -> TestResult {
    let fixture = SdkFrontdoorRuntime::start()?;
    let client = &fixture.client;

    let generation_not_ready = expect_sdk_error(
        client
            .history()
            .query()
            .sourcegraph("type:commit fix")
            .pinned(pin())
            .top_k(5)
            .order(HistoryOrderV1::Recency)
            .execute(),
        "history query without materialized authority should fail",
    )?;
    expect_remote_code(generation_not_ready, "HISTORY_GENERATION_NOT_READY")?;

    let _lexical_receipt = client.search_corpus().publish(&lexical_batch()?)?;
    let producer_unavailable = expect_sdk_error(
        client
            .history()
            .query()
            .sourcegraph("type:commit todo")
            .pinned(pin())
            .top_k(5)
            .order(HistoryOrderV1::Recency)
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
            .order(HistoryOrderV1::Recency)
            .execute(),
        "history diff query should fail when diff shard is absent",
    )?;
    expect_remote_code(shard_unavailable, "HISTORY_SHARD_UNAVAILABLE")?;

    fixture.stop()
}

#[test]
fn sdk_structural_sourcegraph_frontdoor_supports_boolean_and_typed_hole_truth() -> TestResult {
    let fixture = SdkFrontdoorRuntime::start()?;
    let client = &fixture.client;

    publish_sdk_search_corpus_ready(client)?;
    let _structural_receipt = client.structural().publish(&structural_batch()?)?;

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

    fixture.stop()
}

#[test]
fn sdk_dsl_frontdoor_fail_closed_timeout_and_recovery_truth() -> TestResult {
    let fixture = SdkFrontdoorRuntime::start()?;
    let client = &fixture.client;

    publish_sdk_search_corpus_ready(client)?;
    let _structural_receipt = client.structural().publish(&structural_batch()?)?;

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
        SdkError::Remote { code, message, .. }
            if code.as_wire_str() == "QUERY_TIMEOUT"
                && (message.contains("timeout") || message.contains("timed out")) => {}
        other @ (SdkError::Usage(_)
        | SdkError::Protocol(_)
        | SdkError::Serialization(_)
        | SdkError::Transport(_)
        | SdkError::Remote { .. }
        | SdkError::Binding { .. }
        | SdkError::PlaneUnavailable { .. }) => {
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
        SdkError::Remote { code, message, .. }
            if code.as_wire_str() == "STR_INVALID_REQUEST"
                && message.contains("timeout option") => {}
        other @ (SdkError::Usage(_)
        | SdkError::Protocol(_)
        | SdkError::Serialization(_)
        | SdkError::Transport(_)
        | SdkError::Remote { .. }
        | SdkError::Binding { .. }
        | SdkError::PlaneUnavailable { .. }) => {
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
        SdkError::Remote { code, message, .. }
            if code.as_wire_str() == "STR_HOLE_KIND_UNSUPPORTED"
                && message.contains("typed hole kind `lambda`") => {}
        other @ (SdkError::Usage(_)
        | SdkError::Protocol(_)
        | SdkError::Serialization(_)
        | SdkError::Transport(_)
        | SdkError::Remote { .. }
        | SdkError::Binding { .. }
        | SdkError::PlaneUnavailable { .. }) => {
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
        // The OR survivor arrives through the lexical arm without
        // structural captures (TOPT-06: the old timeout-as-Ok wait
        // masked the bindings clause never becoming ready; the
        // assertions below never required them). AND-capture bindings
        // stay guarded by `mixed_boolean` above.
        |response| response.generation == pin() && response.results.len() == 1,
    )?;
    if mixed_or.results.len() != 1 {
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
            return Err(format!(
                "pure-negative root must exclude function_item matches, got {candidate:?}"
            )
            .into());
        }
        if !candidate.bindings.is_empty() {
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
        SdkError::Remote { code, message, .. }
            if code.as_wire_str() == "BRIDGE_TRANSLATE_FAIL"
                && message.contains("patterntype:structural") => {}
        other @ (SdkError::Usage(_)
        | SdkError::Protocol(_)
        | SdkError::Serialization(_)
        | SdkError::Transport(_)
        | SdkError::Remote { .. }
        | SdkError::Binding { .. }
        | SdkError::PlaneUnavailable { .. }) => {
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

    fixture.stop()
}

#[test]
fn sdk_contract_exact_query_request_frontdoors_roundtrip_truth() -> TestResult {
    let fixture = SdkFrontdoorRuntime::start()?;
    let client = &fixture.client;

    let corpus_batch = lexical_batch()?;
    let _corpus_active = publish_and_activate_sdk_search_corpus(client, &corpus_batch)?;
    let _history_receipt = client.history().publish(&history_batch())?;
    let _dirty_receipt = client.runtime().publish_dirty(&dirty_batch())?;
    let _structural_receipt = client.structural().publish(&structural_batch()?)?;

    let lexical_request = TextQueryRequest {
        syntax: TextQuerySyntax::Native,
        query_text: "todo".to_string(),
        constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
        generation: None,
        generation_selector: Some(pinned_selector(pin())),
        top_k: 2,
        cursor: None,
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
        return Err(format!("unexpected contract-exact lexical response: {lexical:?}").into());
    }

    let symbol_request = SymbolQueryRequest {
        syntax: TextQuerySyntax::Sourcegraph,
        query_text: "select:symbol MySdkSymbol".to_string(),
        constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
        generation: None,
        generation_selector: Some(pinned_selector(pin())),
        top_k: 3,
        cursor: None,
    };
    let symbol = wait_for_symbol_query(SOCKET_TIMEOUT, || {
        client.symbol().query_request(symbol_request.clone())
    })?;
    assert_single_symbol_candidate(&symbol)?;

    let history_request = HistoryQueryRequest {
        text_query: TextQueryRequest {
            syntax: TextQuerySyntax::Sourcegraph,
            query_text: "type:commit rev:refs/heads/main author:alice fix".to_string(),
            constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
            generation: None,
            generation_selector: Some(pinned_selector(pin())),
            top_k: 5,
            cursor: None,
        },
        order: HistoryOrderV1::Recency,
        cursor: None,
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
        return Err(format!("unexpected contract-exact history response: {history:?}").into());
    }

    let runtime_request = RuntimeMetadataQueryRequest {
        text_query: TextQueryRequest {
            syntax: TextQuerySyntax::Sourcegraph,
            query_text: "dirty:yes todo".to_string(),
            constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
            generation: None,
            generation_selector: Some(pinned_selector(pin())),
            top_k: 3,
            cursor: None,
        },
        cursor: None,
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
        return Err(format!("unexpected contract-exact runtime response: {runtime:?}").into());
    }

    let structural_request = StructuralQueryRequest {
        text_query: TextQueryRequest {
            syntax: TextQuerySyntax::Sourcegraph,
            query_text:
                r#"repo:repo-sdk path:src/lib.rs lang:rust patterntype:structural "function_item { { :[name.expr] } }""#
                    .to_string(),
            constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
            generation: None,
            generation_selector: Some(pinned_selector(pin())),
            top_k: 2,
            cursor: None,
        },
        cursor: None,
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
        return Err(
            format!("unexpected contract-exact structural response: {structural:?}").into(),
        );
    }

    let semantic_request = SemanticQueryRequest {
        query_text: "quartz".to_string(),
        constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
        generation: None,
        generation_selector: Some(pinned_selector(pin())),
        lexical_scope: Some(TextQueryRequest {
            syntax: TextQuerySyntax::Sourcegraph,
            query_text: "sphinx".to_string(),
            constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
            generation: None,
            generation_selector: Some(pinned_selector(pin())),
            top_k: 2,
            cursor: None,
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
        return Err(format!("unexpected contract-exact semantic response: {semantic:?}").into());
    }

    let hybrid_request = HybridSeedQueryRequest {
        text_query: TextQueryRequest {
            syntax: TextQuerySyntax::Native,
            query_text: "sphinx".to_string(),
            constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
            generation: None,
            generation_selector: Some(pinned_selector(pin())),
            top_k: 2,
            cursor: None,
        },
        semantic_query_text: "quartz".to_string(),
        generation: None,
        generation_selector: Some(pinned_selector(pin())),
        dense_corpora: Vec::new(),
        top_k: 2,
    };
    let hybrid = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || client.search().hybrid_seed_request(hybrid_request.clone()),
        |response| response.generation == pin() && !response.seed_candidates.is_empty(),
    )?;
    let hybrid_top = hybrid
        .seed_candidates
        .first()
        .ok_or_else(|| "missing contract-exact hybrid seed candidate".to_string())?;
    if hybrid.generation != pin()
        || hybrid_top.entity_id != "alpha"
        || hybrid.explanation.summary.is_empty()
    {
        return Err(format!("unexpected contract-exact hybrid-seed response: {hybrid:?}").into());
    }

    fixture.stop()
}

#[test]
fn sdk_search_corpus_frontdoor_promotes_composite_generation_identity() -> TestResult {
    let fixture = SdkFrontdoorRuntime::start()?;
    let client = &fixture.client;

    let initial_status = client.generations().status(repo(), revision())?;
    if initial_status.repo_id != repo()
        || initial_status.revision_id != revision()
        || !initial_status.tracks.is_empty()
    {
        return Err(format!("unexpected initial generation status: {initial_status:?}").into());
    }

    let not_ready = expect_sdk_error(
        client
            .generations()
            .current(repo(), revision(), SearchPlaneTrackKind::Lexical),
        "generation current before activation should fail closed",
    )?;
    expect_remote_code(not_ready, "NOT_READY")?;

    let corpus_batch = lexical_batch()?;
    let composite_active = publish_and_activate_sdk_search_corpus(client, &corpus_batch)?;
    if composite_active.lexical.repo_id != repo()
        || composite_active.lexical.revision_id != revision()
        || composite_active.lexical.track != SearchPlaneTrackKind::Lexical
        || composite_active.semantic.track != SearchPlaneTrackKind::Semantic
        || composite_active.lexical.manifest_generation != generation()
        || composite_active.lexical.manifest_digest != corpus_batch.manifest_digest()
        || composite_active.semantic.manifest_generation != generation()
        || composite_active.semantic.manifest_digest != corpus_batch.manifest_digest()
    {
        return Err(
            format!("unexpected composite activation identity: {composite_active:?}").into(),
        );
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
        || lexical_snapshot.manifest_digest != corpus_batch.manifest_digest()
    {
        return Err(format!("unexpected lexical generation snapshot: {lexical_snapshot:?}").into());
    }

    let final_status = wait_for_sdk_ready(SOCKET_TIMEOUT, || {
        client.generations().status(repo(), revision())
    })?;
    if final_status.repo_id != repo() || final_status.revision_id != revision() {
        return Err(format!("unexpected final generation status: {final_status:?}").into());
    }
    match final_status.tracks.as_slice() {
        [lexical, semantic]
            if lexical.track == SearchPlaneTrackKind::Lexical
                && lexical.manifest_digest == corpus_batch.manifest_digest()
                && semantic.track == SearchPlaneTrackKind::Semantic
                && semantic.manifest_digest == corpus_batch.manifest_digest() => {}
        _ => {
            return Err(format!("unexpected final generation track set: {final_status:?}").into());
        }
    }

    fixture.stop()
}

#[test]
fn sdk_tombstone_only_generation_replaces_active_composite_and_removes_both_query_views()
-> TestResult {
    let fixture = SdkFrontdoorRuntime::start()?;
    let client = &fixture.client;

    let first_generation = lexical_batch()?;
    let _first_active = publish_and_activate_sdk_search_corpus(client, &first_generation)?;

    let _semantic_before_tombstone = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .semantic()
                .query()
                .text("quartz")
                .active(repo(), revision())
                .top_k(3)
                .execute()
        },
        |response| {
            response.generation == pin()
                && response
                    .results
                    .iter()
                    .any(|candidate| candidate.candidate_id == "alpha")
        },
    )?;
    let removed_scope = SourceFileKey {
        source_repo_id: repo(),
        repo_relative_path: RepoRelativePath::new("src/alpha.rs"),
    };
    let tombstone_only = SearchCorpusBatch::delta(
        repo(),
        revision(),
        generation_two(),
        generation(),
        "manifest:lexical-tombstone-only",
    )
    .source_event(lexical_event(
        "fixture:sdk-lexical-tombstone-v2",
        Some("fixture:sdk-lexical-v1"),
    ))
    .tombstone_scope(removed_scope)
    .tombstone_semantic_scope(
        first_generation
            .semantic_replace_scopes()
            .iter()
            .find(|scope| scope.scope.owner_id == "alpha")
            .ok_or_else(|| "first generation missing alpha semantic source".to_string())?
            .scope
            .clone(),
    );

    let expected_active = current_sdk_search_corpus_or_none(client, repo(), revision())?
        .ok_or_else(|| "first composite generation did not become active".to_string())?;
    let (receipt, activation) = client
        .search_corpus()
        .publish_and_activate(&tombstone_only, Some(expected_active))?;
    if !receipt.sealed
        || receipt.generation != generation_two()
        || receipt.manifest_digest.as_deref() != Some(tombstone_only.manifest_digest())
        || receipt.accepted_replace_scopes != 0
        || receipt.accepted_tombstone_scopes != 1
        || receipt.accepted_semantic_tombstone_scopes != 1
        || activation.active.generation.lexical.manifest_generation != generation_two()
        || activation.active.generation.semantic.manifest_generation != generation_two()
        || activation.active.generation.lexical.manifest_digest != tombstone_only.manifest_digest()
        || activation.active.generation.semantic.manifest_digest != tombstone_only.manifest_digest()
    {
        return Err(format!(
            "unexpected tombstone-only sealed composite promotion: receipt={receipt:?} activation={activation:?}"
        )
        .into());
    }

    let lexical = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .lexical()
                .query()
                .native("sphinx")
                .active(repo(), revision())
                .top_k(3)
                .execute()
        },
        |response| response.generation == pin_two(),
    )?;
    if lexical.generation != pin_two()
        || lexical
            .results
            .iter()
            .any(|candidate| candidate.candidate_id == "alpha")
    {
        return Err(format!(
            "tombstone-only lexical generation retained removed alpha scope: {lexical:?}"
        )
        .into());
    }

    let semantic = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .semantic()
                .query()
                .text("quartz")
                .active(repo(), revision())
                .top_k(3)
                .execute()
        },
        |response| response.generation == pin_two(),
    )?;
    if semantic.generation != pin_two()
        || semantic
            .results
            .iter()
            .any(|candidate| candidate.candidate_id == "alpha")
    {
        return Err(format!(
            "tombstone-only semantic generation retained removed alpha scope: {semantic:?}"
        )
        .into());
    }

    fixture.stop()
}

#[test]
fn sdk_builder_variant_frontdoors_route_native_inline_vector_and_pinned_truth() -> TestResult {
    let fixture = SdkFrontdoorRuntime::start()?;
    let client = &fixture.client;

    let corpus_batch = lexical_batch()?;
    let _corpus_active = publish_and_activate_sdk_search_corpus(client, &corpus_batch)?;
    let _dirty_receipt = client.runtime().publish_dirty(&dirty_batch())?;

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
        return Err(
            format!("unexpected semantic inline-vector response: {semantic_inline:?}").into(),
        );
    }

    let hybrid_inline = wait_for_sdk_observation(
        SOCKET_TIMEOUT,
        || {
            client
                .search()
                .hybrid_seed()
                .native("sphinx")
                .semantic_text("quartz")
                .pinned(pin())
                .top_k(2)
                .execute()
        },
        |response| response.generation == pin() && !response.seed_candidates.is_empty(),
    )?;
    let hybrid_inline_top = hybrid_inline
        .seed_candidates
        .first()
        .ok_or_else(|| "missing hybrid inline-vector seed candidate".to_string())?;
    if hybrid_inline.generation != pin()
        || hybrid_inline_top.entity_id != "alpha"
        || hybrid_inline.explanation.summary.is_empty()
    {
        return Err(
            format!("unexpected hybrid inline-vector seed response: {hybrid_inline:?}").into(),
        );
    }

    fixture.stop()
}

#[test]
fn sdk_multi_generation_restart_frontdoor_preserves_pinned_and_flips_active_composite_corpus()
-> TestResult {
    let complex_timeout = Duration::from_secs(30);
    let dir = quanta_index_searchd_harness::private_tempdir()?;
    let state_root = dir.path().to_path_buf();
    let fixture = SdkFrontdoorRuntime::start_at(&state_root)?;
    let client = &fixture.client;

    let corpus_batch_v1 = lexical_batch()?;
    let _corpus_active_v1 = publish_and_activate_sdk_search_corpus(client, &corpus_batch_v1)?;
    let _structural_receipt_v1 = client.structural().publish(&structural_batch()?)?;

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
        return Err(format!("unexpected active v1 lexical response: {lexical_active_v1:?}").into());
    }

    let corpus_batch_v2 = lexical_batch_two()?;
    let _corpus_active_v2 = publish_and_activate_sdk_search_corpus(client, &corpus_batch_v2)?;
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
        return Err(
            format!("unexpected pinned v2 structural response: {structural_pinned_v2:?}").into(),
        );
    }

    fixture.stop()?;
    let fixture = SdkFrontdoorRuntime::start_at(&state_root)?;
    let client = &fixture.client;

    let lexical_active_after_restart = wait_for_sdk_observation(
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
    let lexical_active_after_restart_candidate = lexical_active_after_restart
        .results
        .first()
        .ok_or_else(|| "missing restarted active v1 lexical candidate".to_string())?;
    if lexical_active_after_restart.generation != pin_two()
        || lexical_active_after_restart_candidate.candidate_id != "chunk-dirty-v2"
    {
        return Err(format!(
            "unexpected restarted active v2 lexical response: {lexical_active_after_restart:?}"
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
        return Err(format!(
            "unexpected restarted pinned v2 structural response: \
             {structural_pinned_v2_after_restart:?}"
        )
        .into());
    }

    let lexical_snapshot_v2 = wait_for_sdk_ready(complex_timeout, || {
        client
            .generations()
            .current(repo(), revision(), SearchPlaneTrackKind::Lexical)
    })?;
    if lexical_snapshot_v2.manifest_generation != generation_two()
        || lexical_snapshot_v2.manifest_digest != "manifest:lexical-v2"
    {
        return Err(format!(
            "unexpected post-restart lexical generation snapshot: {lexical_snapshot_v2:?}"
        )
        .into());
    }

    let semantic_snapshot_v2 = wait_for_sdk_ready(complex_timeout, || {
        client
            .generations()
            .current(repo(), revision(), SearchPlaneTrackKind::Semantic)
    })?;
    if semantic_snapshot_v2.manifest_generation != lexical_snapshot_v2.manifest_generation
        || semantic_snapshot_v2.manifest_digest != lexical_snapshot_v2.manifest_digest
    {
        return Err(format!(
            "restart split the active composite corpus: lexical={lexical_snapshot_v2:?} semantic={semantic_snapshot_v2:?}"
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
        return Err(format!(
            "unexpected pinned v1 lexical response after flip: {lexical_pinned_v1_after_flip:?}"
        )
        .into());
    }

    fixture.stop()
}

#[test]
fn sdk_binary_process_dsl_roundtrip() -> TestResult {
    let dir = quanta_index_searchd_harness::private_tempdir()?;
    let runtime = SearchdBinaryProcess::start(dir.path())?;
    let result = (|| -> TestResult {
        let client = runtime.connect()?;
        // The repo-meta overlay is part of the sealed contract: published
        // before the seal, never into the sealed generation (QI-BB-030).
        let _repo_meta_receipt = client.history().publish_repo_meta(&repo_meta_batch())?;
        let batch = lexical_frontdoor_matrix_batch()?;
        let active = publish_and_activate_sdk_search_corpus(&client, &batch)?;
        if active.lexical.manifest_generation != generation()
            || active.semantic.manifest_generation != generation()
            || active.lexical.manifest_digest != batch.manifest_digest()
            || active.semantic.manifest_digest != batch.manifest_digest()
        {
            return Err(format!(
                "binary process promoted an unexpected composite generation: {active:?}"
            )
            .into());
        }

        let response = wait_for_sdk_observation(
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
        let paths = response
            .results
            .iter()
            .map(|candidate| candidate.repo_relative_path.as_str().to_string())
            .collect::<BTreeSet<_>>();
        if response.generation != pin()
            || paths != BTreeSet::from(["src/alpha.rs".to_string(), "src/beta.rs".to_string()])
        {
            return Err(format!(
                "binary process DSL query diverged: response={response:?} paths={paths:?}"
            )
            .into());
        }

        let predicate = wait_for_sdk_observation(
            SOCKET_TIMEOUT,
            || {
                client
                    .lexical()
                    .query()
                    .sourcegraph("repo:has.meta(license:apache-2.0) shared_oracle_needle")
                    .active(repo(), revision())
                    .top_k(5)
                    .execute()
            },
            // The predicate query correlates two source rows (TOPT-06:
            // the old timeout-as-Ok wait masked this `len == 1` never
            // becoming ready; the assertion below always wanted both).
            |response| response.generation == pin() && response.results.len() == 2,
        )?;
        let predicate_paths = predicate
            .results
            .iter()
            .map(|candidate| candidate.repo_relative_path.as_str().to_string())
            .collect::<Vec<_>>();
        if predicate.generation != pin()
            || predicate_paths
                != [
                    "src/recency_a.rs".to_string(),
                    "src/recency_gate.rs".to_string(),
                ]
        {
            return Err(format!(
                "binary process predicate query lost source-repo correlation: response={predicate:?}"
            )
            .into());
        }
        Ok(())
    })();
    let stop = runtime.stop();
    result.and(stop)
}

// ---------------------------------------------------------------------------
// TH-1 adapter proofs (TOPT-06): the SDK wait adapters classify by wire
// code and fail closed. Scripted closures and a virtual clock exercise
// the real adapter without daemon startup or wall-clock waits.
// ---------------------------------------------------------------------------

#[derive(Default)]
struct ScriptedWaitTicker {
    elapsed: Cell<Duration>,
}

impl WaitTicker for ScriptedWaitTicker {
    fn now(&self) -> Duration {
        self.elapsed.get()
    }

    fn sleep(&self, duration: Duration) {
        self.elapsed
            .set(self.elapsed.get().saturating_add(duration));
    }
}

/// A scripted remote refusal behind no transport at all.
fn scripted_remote(code: SearchPlaneErrorCodeV2) -> SdkError {
    SdkError::Remote {
        code,
        message: "scripted".to_string(),
        repair: None,
    }
}

#[test]
fn sdk_wait_never_ready_script_returns_typed_timeout() {
    let ticker = ScriptedWaitTicker::default();
    let calls = AtomicU64::new(0);
    let error = wait_for_sdk_ready_with_ticker(&ticker, Duration::from_millis(25), || {
        let _prior = calls.fetch_add(1, Ordering::SeqCst);
        Err::<(), SdkError>(scripted_remote(SearchPlaneErrorCodeV2::NotReady))
    })
    .expect_err("a never-ready script fails");
    let WaitError::Timeout(timeout) = error else {
        panic!("a never-ready script times out typed, got {error:?}");
    };
    assert!(
        timeout.attempts > 1,
        "a virtual-clock NOT_READY script exercises retry"
    );
    assert_eq!(calls.load(Ordering::SeqCst), timeout.attempts);
    assert!(
        timeout.expected.contains("sdk_frontdoor.rs"),
        "the timeout names its wait call site: {}",
        timeout.expected
    );
    let last = timeout.last.expect("the last NOT_READY is evidence");
    assert!(
        last.contains("NotReady"),
        "the last observation names the code: {last}"
    );
}

#[test]
fn sdk_wait_non_retryable_error_returns_terminal_at_once() {
    let ticker = ScriptedWaitTicker::default();
    let calls = AtomicU64::new(0);
    let error = wait_for_sdk_ready_with_ticker(&ticker, Duration::from_millis(25), || {
        let _prior = calls.fetch_add(1, Ordering::SeqCst);
        Err::<(), SdkError>(scripted_remote(SearchPlaneErrorCodeV2::Lexical(
            LexicalErrorCode::QueryTimeout,
        )))
    })
    .expect_err("a terminal error fails");
    assert!(
        matches!(error, WaitError::Terminal(_)),
        "a non-retryable code is terminal, never retried: {error:?}"
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "a terminal error returns without sleeping"
    );
}

#[test]
fn sdk_wait_not_ready_then_ready_recovers() {
    let ticker = ScriptedWaitTicker::default();
    let calls = AtomicU64::new(0);
    let value = wait_for_sdk_ready_with_ticker(&ticker, Duration::from_secs(5), || {
        let call = calls.fetch_add(1, Ordering::SeqCst).saturating_add(1);
        if call < 3 {
            Err(scripted_remote(SearchPlaneErrorCodeV2::NotReady))
        } else {
            Ok("ready")
        }
    })
    .expect("NOT_READY then ready returns the value");
    assert_eq!(value, "ready");
    assert_eq!(calls.load(Ordering::SeqCst), 3);
}

// L2: real SDK + sockets + durable journal/catalog + paired storage. The
// original source publication survives retargeting and a runtime restart.
#[test]
fn l2_source_replay_keeps_original_publication_through_sdk_activation_and_restart() -> TestResult {
    use quanta_index_contract::{IngestObservationStatus, SourcePublicationEvent};
    let root = quanta_index_searchd_harness::private_tempdir()?;
    let fixture = SdkFrontdoorRuntime::start_at(root.path())?;
    let event = SourcePublicationEvent {
        stream_id: "l2-sdk-stream".into(),
        event_id: "l2-sdk-event".into(),
        expected_base_event_id: None,
        payload_sha256: [0; 32], // The SDK commits the actual empty payload.
    };
    let original = SearchCorpusBatch::replace_generation(
        repo(),
        revision(),
        ManifestGeneration::new(1),
        "manifest:l2-original",
    )
    .source_event(event.clone());
    let first = fixture
        .client
        .producer()
        .publish_search_corpus_observed(&original)?;
    assert!(first.receipt.applied);
    assert!(first.receipt.durable_sequence > 0);
    let requested_revision = RevisionId::new("l2-retargeted-revision")?;
    let retargeted = SearchCorpusBatch::replace_generation(
        repo(),
        requested_revision.clone(),
        ManifestGeneration::new(99),
        "manifest:l2-retargeted",
    )
    .source_event(event);
    assert_ne!(retargeted.batch_digest()?, first.receipt.batch_digest);
    let (replayed, activation) = fixture
        .client
        .search_corpus()
        .publish_and_activate_observed(&retargeted, None)?;
    assert_eq!(replayed.publication, first.publication);
    assert_eq!(replayed.receipt, first.receipt.clone().replayed());
    let observation = replayed
        .observation
        .as_ref()
        .ok_or("missing replay observation")?;
    assert_eq!(observation.status, IngestObservationStatus::Replayed);
    assert_eq!(observation.revision_id, requested_revision);
    assert_eq!(observation.generation, ManifestGeneration::new(99));
    assert!(observation.semantic.is_none());
    assert!(observation.lexical_build_ns.is_none());
    assert_eq!(
        activation.active.generation.lexical,
        first.publication.target
    );
    assert_eq!(
        activation.active.generation.semantic.manifest_generation,
        ManifestGeneration::new(1)
    );
    assert_eq!(
        current_sdk_search_corpus_or_none(&fixture.client, repo(), revision())?,
        Some(activation.active.clone())
    );
    assert_eq!(
        current_sdk_search_corpus_or_none(&fixture.client, repo(), requested_revision.clone())?,
        None
    );
    fixture.stop()?;

    let restarted = SdkFrontdoorRuntime::start_at(root.path())?;
    let replayed = restarted
        .client
        .producer()
        .publish_search_corpus_observed(&retargeted)?;
    assert_eq!(replayed.publication, first.publication);
    assert_eq!(replayed.receipt, first.receipt.replayed());
    assert_eq!(
        current_sdk_search_corpus_or_none(&restarted.client, repo(), revision())?,
        Some(activation.active)
    );
    assert_eq!(
        current_sdk_search_corpus_or_none(&restarted.client, repo(), requested_revision)?,
        None
    );
    restarted.stop()
}
