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

fn code_search_uses_separate_fixture<T>() -> Result<T, SdkError> {
    Err(SdkError::Protocol(
        "CodeSearch is exercised by its own lexical fixture".into(),
    ))
}

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
        SearchPlaneIngestIpcResponse::SourcePublicationUploadAck(_) => {
            Err("ingest publish returned an upload acknowledgement without a receipt".into())
        }
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
    Ok(batch.replace_scope(
        scope.coverage,
        scope.source_bytes,
        scope.chunks,
        scope.symbols,
    ))
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
        | SdkError::AfterPublish { .. }
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

#[path = "sdk_frontdoor/binary_tests.rs"]
mod binary_tests;
#[path = "sdk_frontdoor/failure_tests.rs"]
mod failure_tests;
#[path = "sdk_frontdoor/lifecycle_tests.rs"]
mod lifecycle_tests;
#[path = "sdk_frontdoor/matrix_tests.rs"]
mod matrix_tests;
#[path = "sdk_frontdoor/publication_tests.rs"]
mod publication_tests;
#[path = "sdk_frontdoor/replay_tests.rs"]
mod replay_tests;
#[path = "sdk_frontdoor/search_tests.rs"]
mod search_tests;
#[path = "sdk_frontdoor/wait_tests.rs"]
mod wait_tests;
