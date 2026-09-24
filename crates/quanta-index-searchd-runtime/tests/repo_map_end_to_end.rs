//! End-to-end integration: preload repo-map owner state in searchd runtime and
//! query it through the UDS search-plane IPC.

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
    clippy::panic_in_result_fn,
    reason = "integration tests use Result-returning setup with assertion-style validation"
)]
#![expect(
    clippy::wildcard_enum_match_arm,
    reason = "integration response checks intentionally collapse non-target variants"
)]

use std::error::Error;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use quanta_index_contract::lex::{LanguageCode, SymbolKindCode};
use quanta_index_contract::{
    FileId, ManifestGeneration, RepoId, RepoMapActivateGenerationRequestV2, RepoMapChunkExactness,
    RepoMapContainsEdge, RepoMapDocType, RepoMapExactnessSummary, RepoMapFocusSubjectDto,
    RepoMapGraphCoverage, RepoMapGraphCoverageClass, RepoMapItemIndexAvailability,
    RepoMapMutationPhaseV2, RepoMapNode, RepoMapNodeRef, RepoMapOwnsChunkEdge,
    RepoMapPublishBundleRequestV2, RepoMapQueryRequest, RepoMapRedactionState, RepoMapSourceBundle,
    RepoRelativePath, RevisionId, SearchPlaneControlIpcRequest,
    SearchPlaneControlIpcRequestEnvelope, SearchPlaneControlIpcResponse,
    SearchPlaneControlIpcResponseEnvelope, SearchPlaneIngestIpcRequest,
    SearchPlaneIngestIpcRequestEnvelope, SearchPlaneIngestIpcResponse,
    SearchPlaneIngestIpcResponseEnvelope, SearchPlaneQueryIpcRequest,
    SearchPlaneQueryIpcRequestEnvelope, SearchPlaneQueryIpcResponse,
    SearchPlaneQueryIpcResponseEnvelope, SymbolId,
};
use quanta_index_ipc::send_request;
use quanta_index_searchd_harness::E2eRuntime;

type TestResult = Result<(), Box<dyn Error>>;
const READINESS_TIMEOUT: Duration = Duration::from_secs(5);

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
    control_socket: std::path::PathBuf,
    ingest_socket: std::path::PathBuf,
}

impl ScenarioFixture {
    fn boot() -> Result<Self, Box<dyn Error>> {
        let mut runtime = E2eRuntime::boot()?;
        // Eager start surfaces a boot refusal here and binds all three
        // sockets before any byte is published.
        runtime.start()?;
        Self::wrap(runtime)
    }

    /// Acknowledged restart: the old driver is signalled and joined (dropping
    /// the old runtime and releasing the state-root lease) before a fresh
    /// runtime is built over the same state root — no sleep-based handoff.
    fn restart(self) -> Result<Self, Box<dyn Error>> {
        let mut runtime = self.runtime.reopen();
        runtime.start()?;
        Self::wrap(runtime)
    }

    fn wrap(runtime: E2eRuntime) -> Result<Self, Box<dyn Error>> {
        let (query_socket, control_socket, ingest_socket) = {
            let (query, control, ingest) = runtime
                .socket_paths()
                .ok_or_else(|| "fixture: driver started without socket paths".to_string())?;
            (
                query.to_path_buf(),
                control.to_path_buf(),
                ingest.to_path_buf(),
            )
        };
        Ok(Self {
            runtime,
            query_socket,
            control_socket,
            ingest_socket,
        })
    }
}

fn repo() -> RepoId {
    RepoId::new("repo-repomap-e2e").expect("static fixture ID satisfies canonical policy")
}

fn revision() -> RevisionId {
    RevisionId::new("rev-repomap-e2e").expect("static fixture ID satisfies canonical policy")
}

fn generation() -> ManifestGeneration {
    ManifestGeneration::new(11)
}

fn rust_language() -> Result<LanguageCode, Box<dyn Error>> {
    LanguageCode::new("rust").map_err(|err| -> Box<dyn Error> {
        format!("invalid hard-coded test language code: {err}").into()
    })
}

fn symbol_kind(name: &str) -> Result<SymbolKindCode, Box<dyn Error>> {
    SymbolKindCode::new(name)
        .map_err(|err| format!("invalid hard-coded test symbol kind `{name}`: {err}").into())
}

fn send_query_request(
    socket: &Path,
    request: &SearchPlaneQueryIpcRequestEnvelope,
) -> Result<SearchPlaneQueryIpcResponseEnvelope, quanta_index_ipc::IpcError> {
    send_request(socket, request, quanta_index_ipc::ClientIoPolicy::default())
}

fn send_control_request(
    socket: &Path,
    request: &SearchPlaneControlIpcRequestEnvelope,
) -> Result<SearchPlaneControlIpcResponseEnvelope, quanta_index_ipc::IpcError> {
    send_request(socket, request, quanta_index_ipc::ClientIoPolicy::default())
}

fn send_ingest_request(
    socket: &Path,
    request: &SearchPlaneIngestIpcRequestEnvelope,
) -> Result<SearchPlaneIngestIpcResponseEnvelope, quanta_index_ipc::IpcError> {
    send_request(socket, request, quanta_index_ipc::ClientIoPolicy::default())
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

fn repo_map_bundle() -> Result<RepoMapSourceBundle, Box<dyn Error>> {
    Ok(RepoMapSourceBundle::new(
        repo(),
        revision(),
        generation(),
        "1".repeat(64),
        "repomap-snapshot-11",
        1,
        "d".repeat(64),
        RepoMapGraphCoverage {
            item_index_availability: RepoMapItemIndexAvailability::Available,
            graph_coverage_class: RepoMapGraphCoverageClass::Complete,
        },
        RepoMapExactnessSummary::Exact,
        RepoMapRedactionState::Unredacted,
    )
    .with_node(RepoMapNode::File(quanta_index_contract::RepoMapFileNode {
        file_id: FileId::new("file://src/lib.rs"),
        repo_relative_path: RepoRelativePath::new("src/lib.rs"),
        line_count: 110,
    }))
    .with_node(RepoMapNode::File(quanta_index_contract::RepoMapFileNode {
        file_id: FileId::new("file://src/service/mod.rs"),
        repo_relative_path: RepoRelativePath::new("src/service/mod.rs"),
        line_count: 170,
    }))
    .with_node(RepoMapNode::File(quanta_index_contract::RepoMapFileNode {
        file_id: FileId::new("file://tests/repo_map.rs"),
        repo_relative_path: RepoRelativePath::new("tests/repo_map.rs"),
        line_count: 70,
    }))
    .with_node(RepoMapNode::Symbol(
        quanta_index_contract::RepoMapSymbolNode {
            symbol_id: SymbolId::new("symbol://alpha"),
            owner_path: RepoRelativePath::new("src/lib.rs"),
            local_name: "Alpha".to_string(),
            qualified_name: "src::lib::Alpha".to_string(),
            symbol_kind: symbol_kind("struct")?,
        },
    ))
    .with_node(RepoMapNode::Symbol(
        quanta_index_contract::RepoMapSymbolNode {
            symbol_id: SymbolId::new("symbol://beta"),
            owner_path: RepoRelativePath::new("src/service/mod.rs"),
            local_name: "Beta".to_string(),
            qualified_name: "src::service::Beta".to_string(),
            symbol_kind: symbol_kind("struct")?,
        },
    ))
    .with_node(RepoMapNode::Symbol(
        quanta_index_contract::RepoMapSymbolNode {
            symbol_id: SymbolId::new("symbol://gamma"),
            owner_path: RepoRelativePath::new("tests/repo_map.rs"),
            local_name: "Gamma".to_string(),
            qualified_name: "tests::repo_map::Gamma".to_string(),
            symbol_kind: symbol_kind("struct")?,
        },
    ))
    .with_node(RepoMapNode::Chunk(
        quanta_index_contract::RepoMapChunkNode {
            chunk_id: quanta_index_contract::ChunkId::new("chunk://alpha"),
            owner_path: RepoRelativePath::new("src/lib.rs"),
            language: rust_language()?,
            start_byte: 0,
            end_byte: 128,
            start_line: 1,
            end_line: 12,
            token_count: 64,
            preview_text: "Alpha library owner index".to_string(),
            exactness: RepoMapChunkExactness::Exact,
        },
    ))
    .with_node(RepoMapNode::Chunk(
        quanta_index_contract::RepoMapChunkNode {
            chunk_id: quanta_index_contract::ChunkId::new("chunk://beta"),
            owner_path: RepoRelativePath::new("src/service/mod.rs"),
            language: rust_language()?,
            start_byte: 129,
            end_byte: 256,
            start_line: 13,
            end_line: 28,
            token_count: 96,
            preview_text: "Beta service owner query entrypoint".to_string(),
            exactness: RepoMapChunkExactness::Exact,
        },
    ))
    .with_node(RepoMapNode::Chunk(
        quanta_index_contract::RepoMapChunkNode {
            chunk_id: quanta_index_contract::ChunkId::new("chunk://repomap-test"),
            owner_path: RepoRelativePath::new("tests/repo_map.rs"),
            language: rust_language()?,
            start_byte: 257,
            end_byte: 320,
            start_line: 29,
            end_line: 35,
            token_count: 40,
            preview_text: "repo map integration test".to_string(),
            exactness: RepoMapChunkExactness::Approximate,
        },
    ))
    .with_edge(quanta_index_contract::RepoMapEdge::Contains(
        RepoMapContainsEdge {
            container: RepoMapNodeRef::File(FileId::new("file://src/lib.rs")),
            contained: RepoMapNodeRef::Symbol(SymbolId::new("symbol://alpha")),
        },
    ))
    .with_edge(quanta_index_contract::RepoMapEdge::Contains(
        RepoMapContainsEdge {
            container: RepoMapNodeRef::File(FileId::new("file://src/service/mod.rs")),
            contained: RepoMapNodeRef::Symbol(SymbolId::new("symbol://beta")),
        },
    ))
    .with_edge(quanta_index_contract::RepoMapEdge::Call(
        quanta_index_contract::RepoMapCallEdge {
            caller: RepoMapNodeRef::Symbol(SymbolId::new("symbol://beta")),
            callee: RepoMapNodeRef::Symbol(SymbolId::new("symbol://alpha")),
        },
    ))
    .with_edge(quanta_index_contract::RepoMapEdge::Call(
        quanta_index_contract::RepoMapCallEdge {
            caller: RepoMapNodeRef::Symbol(SymbolId::new("symbol://beta")),
            callee: RepoMapNodeRef::Symbol(SymbolId::new("symbol://gamma")),
        },
    ))
    .with_edge(quanta_index_contract::RepoMapEdge::Import(
        quanta_index_contract::RepoMapImportEdge {
            importer: RepoMapNodeRef::File(FileId::new("file://src/service/mod.rs")),
            imported: RepoMapNodeRef::File(FileId::new("file://src/lib.rs")),
        },
    ))
    .with_edge(quanta_index_contract::RepoMapEdge::OwnsChunk(
        RepoMapOwnsChunkEdge {
            owner: RepoMapNodeRef::Symbol(SymbolId::new("symbol://alpha")),
            chunk: RepoMapNodeRef::Chunk(quanta_index_contract::ChunkId::new("chunk://alpha")),
        },
    ))
    .with_edge(quanta_index_contract::RepoMapEdge::OwnsChunk(
        RepoMapOwnsChunkEdge {
            owner: RepoMapNodeRef::Symbol(SymbolId::new("symbol://beta")),
            chunk: RepoMapNodeRef::Chunk(quanta_index_contract::ChunkId::new("chunk://beta")),
        },
    ))
    .with_edge(quanta_index_contract::RepoMapEdge::OwnsChunk(
        RepoMapOwnsChunkEdge {
            owner: RepoMapNodeRef::File(FileId::new("file://tests/repo_map.rs")),
            chunk: RepoMapNodeRef::Chunk(quanta_index_contract::ChunkId::new(
                "chunk://repomap-test",
            )),
        },
    )))
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
                subject_identity: "symbol://beta".to_string(),
                subject_doc_type: RepoMapDocType::Symbol,
            }],
        }),
    }
}

// RepoMap bundle ingest flows over the ingest IPC, not the control IPC.
fn repo_map_ingest_envelope() -> Result<SearchPlaneIngestIpcRequestEnvelope, Box<dyn Error>> {
    Ok(SearchPlaneIngestIpcRequestEnvelope {
        request_id: 75,
        payload: SearchPlaneIngestIpcRequest::PublishRepoMapBundleV2(
            RepoMapPublishBundleRequestV2::new(repo_map_bundle()?)?,
        ),
    })
}

fn assert_repo_map_transport_surface(
    repo_map: &quanta_index_contract::RepoMapQueryResponse,
) -> TestResult {
    if repo_map.repo_id != repo()
        || repo_map.revision_id != revision()
        || repo_map.manifest_generation != generation()
        || repo_map.snapshot_meta.snapshot_id != "repomap-snapshot-11"
        || repo_map.entries.is_empty()
    {
        return Err(format!("unexpected repo-map transport response: {repo_map:?}").into());
    }
    Ok(())
}

#[test]
fn repo_map_query_roundtrip_through_searchd_socket() -> TestResult {
    let fixture = ScenarioFixture::boot()?;
    let query_socket = fixture.query_socket.clone();
    let control_socket = fixture.control_socket.clone();
    let ingest_socket = fixture.ingest_socket;
    let source = repo_map_bundle()?;
    let publish_request = RepoMapPublishBundleRequestV2::new(source.clone())?;
    let publish_envelope = SearchPlaneIngestIpcRequestEnvelope {
        request_id: 75,
        payload: SearchPlaneIngestIpcRequest::PublishRepoMapBundleV2(publish_request.clone()),
    };
    let ingest = send_ingest_request(&ingest_socket, &publish_envelope)
        .map_err(|err| format!("repo-map ingest request failed: {err}"))?;
    let publish = match ingest.payload {
        SearchPlaneIngestIpcResponse::RepoMapTerminalReceiptV2(receipt)
            if receipt.phase == RepoMapMutationPhaseV2::Publish
                && !receipt.mutation.replayed
                && receipt.source_bundle_digest == publish_request.source_bundle_digest =>
        {
            receipt
        }
        other => {
            return Err(format!("repo-map V2 publish did not ack: {other:?}").into());
        }
    };
    let replay = send_ingest_request(&ingest_socket, &publish_envelope)
        .map_err(|err| format!("repo-map V2 replay request failed: {err}"))?;
    let mut expected_replay = publish.clone();
    expected_replay.mutation.replayed = true;
    if replay.payload != SearchPlaneIngestIpcResponse::RepoMapTerminalReceiptV2(expected_replay) {
        return Err(format!("repo-map V2 replay changed the terminal receipt: {replay:?}").into());
    }

    let mut substituted = source.clone();
    substituted.snapshot_id.push_str("-foreign");
    let refusal = send_ingest_request(
        &ingest_socket,
        &SearchPlaneIngestIpcRequestEnvelope {
            request_id: 76,
            payload: SearchPlaneIngestIpcRequest::PublishRepoMapBundleV2(
                RepoMapPublishBundleRequestV2::new(substituted)?,
            ),
        },
    )?;
    if !matches!(
        refusal.payload,
        SearchPlaneIngestIpcResponse::Error(ref error)
            if error.code == quanta_index_contract::SearchPlaneErrorCodeV2::CandidateCommitmentConflict
    ) {
        return Err(format!("repo-map V2 substituted source was not refused: {refusal:?}").into());
    }

    let activate = send_control_request(
        &control_socket,
        &SearchPlaneControlIpcRequestEnvelope {
            request_id: 77,
            payload: SearchPlaneControlIpcRequest::RepoMapActivateV2(
                RepoMapActivateGenerationRequestV2::for_bundle(&source)?,
            ),
        },
    )
    .map_err(|err| format!("repo-map activate request failed: {err}"))?;
    if !matches!(activate.payload,
        SearchPlaneControlIpcResponse::RepoMapTerminalReceiptV2(ref receipt)
            if receipt.phase == RepoMapMutationPhaseV2::Activate
                && receipt.source_bundle_digest == publish.source_bundle_digest
                && receipt.mutation.new_candidate_commitment == publish.mutation.new_candidate_commitment
    ) {
        return Err(format!("repo-map V2 activate did not bind publish: {activate:?}").into());
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
        return Err("repo-map query path never became ready".into());
    }

    let response = send_query_request(&query_socket, &repo_map_request())
        .map_err(|err| format!("repo-map query request failed: {err}"))?;
    let repo_map = match response.payload {
        SearchPlaneQueryIpcResponse::RepoMapQuery(repo_map) => repo_map,
        other => {
            return Err(format!("expected RepoMapQuery response, got {other:?}").into());
        }
    };
    assert_repo_map_transport_surface(&repo_map)?;

    Ok(())
}

#[test]
fn repo_map_query_survives_runtime_restart_from_persisted_state() -> TestResult {
    let fixture = ScenarioFixture::boot()?;
    let control_socket = fixture.control_socket.clone();
    let ingest_socket = fixture.ingest_socket.clone();
    let source = repo_map_bundle()?;
    let publish_request = RepoMapPublishBundleRequestV2::new(source.clone())?;
    let publish_envelope = SearchPlaneIngestIpcRequestEnvelope {
        request_id: 75,
        payload: SearchPlaneIngestIpcRequest::PublishRepoMapBundleV2(publish_request),
    };
    let ingest = send_ingest_request(&ingest_socket, &publish_envelope)
        .map_err(|err| format!("repo-map ingest request failed: {err}"))?;
    let publish = match ingest.payload {
        SearchPlaneIngestIpcResponse::RepoMapTerminalReceiptV2(receipt)
            if receipt.phase == RepoMapMutationPhaseV2::Publish && !receipt.mutation.replayed =>
        {
            receipt
        }
        other => {
            return Err(format!("repo-map V2 seed publish failed: {other:?}").into());
        }
    };
    let activate_envelope = SearchPlaneControlIpcRequestEnvelope {
        request_id: 76,
        payload: SearchPlaneControlIpcRequest::RepoMapActivateV2(
            RepoMapActivateGenerationRequestV2::for_bundle(&source)?,
        ),
    };
    let activate = send_control_request(&control_socket, &activate_envelope)
        .map_err(|err| format!("repo-map activate request failed: {err}"))?;
    let activation = match activate.payload {
        SearchPlaneControlIpcResponse::RepoMapTerminalReceiptV2(receipt)
            if receipt.phase == RepoMapMutationPhaseV2::Activate
                && receipt.source_bundle_digest == publish.source_bundle_digest =>
        {
            receipt
        }
        other => {
            return Err(format!("repo-map V2 seed activation failed: {other:?}").into());
        }
    };

    let fixture = fixture.restart()?;
    let query_socket = fixture.query_socket;
    let control_socket = fixture.control_socket;
    let ingest_socket = fixture.ingest_socket;
    let replay = send_ingest_request(&ingest_socket, &publish_envelope)?;
    let mut expected_publish_replay = publish;
    expected_publish_replay.mutation.replayed = true;
    if replay.payload
        != SearchPlaneIngestIpcResponse::RepoMapTerminalReceiptV2(expected_publish_replay)
    {
        return Err(format!("repo-map V2 restart publish replay drifted: {replay:?}").into());
    }
    let activate_replay = send_control_request(&control_socket, &activate_envelope)?;
    let mut expected_activation_replay = activation;
    expected_activation_replay.mutation.replayed = true;
    if activate_replay.payload
        != SearchPlaneControlIpcResponse::RepoMapTerminalReceiptV2(expected_activation_replay)
    {
        return Err(
            format!("repo-map V2 restart activation replay drifted: {activate_replay:?}").into(),
        );
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
        return Err("repo-map persisted query path never became ready".into());
    }

    let response = send_query_request(&query_socket, &repo_map_request())
        .map_err(|err| format!("repo-map query request after restart failed: {err}"))?;
    let repo_map = match response.payload {
        SearchPlaneQueryIpcResponse::RepoMapQuery(repo_map) => repo_map,
        other => {
            return Err(
                format!("expected RepoMapQuery response after restart, got {other:?}").into(),
            );
        }
    };
    assert_repo_map_transport_surface(&repo_map)?;

    Ok(())
}

#[test]
fn repo_map_query_without_materialized_snapshot_fails_closed() -> TestResult {
    let fixture = ScenarioFixture::boot()?;
    let query_socket = fixture.query_socket;

    let response = send_query_request(&query_socket, &repo_map_request())
        .map_err(|err| format!("repo-map missing-snapshot query failed: {err}"))?;
    let err = match response.payload {
        SearchPlaneQueryIpcResponse::Error(err) => err,
        other => {
            return Err(format!("expected error response, got {other:?}").into());
        }
    };
    assert_eq!(err.code.as_wire_str(), "NOT_FOUND");
    assert!(err.message.contains("no activated generation"));

    Ok(())
}

#[test]
fn cross_socket_requests_fail_closed() -> TestResult {
    let fixture = ScenarioFixture::boot()?;
    let query_socket = fixture.query_socket;
    let control_socket = fixture.control_socket;

    match send_ingest_request(&query_socket, &repo_map_ingest_envelope()?) {
        Ok(unexpected) => {
            return Err(format!(
                "ingest envelope on query socket must fail closed, got {unexpected:?}"
            )
            .into());
        }
        Err(err) => check_connection_fatal(err)?,
    }

    match send_query_request(&control_socket, &repo_map_request()) {
        Ok(unexpected) => {
            return Err(format!(
                "query envelope on control socket must fail closed, got {unexpected:?}"
            )
            .into());
        }
        Err(err) => check_connection_fatal(err)?,
    }

    Ok(())
}
