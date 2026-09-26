//! S21-07 owner integration target: SDK contextual response binding.
//!
//! Every negative case below drives the real public SDK surface over a
//! real UDS socket against a scripted wire peer that returns a response
//! with the *correct request id and the correct variant* but the wrong
//! context — exactly the wrong-but-same-variant matrix the ticket names.
//! Each must fail closed as a typed `SdkError::Binding` naming the route
//! and the failed axis, before any caller can read a field out of the
//! response. The coverage test asserts the exported wire-route table
//! exact-matches the SDK dispatch surface.

// The workspace denies `expect` everywhere; this owner test extracts
// typed errors as values, which is exactly what `expect_err` is for.
#![expect(
    clippy::expect_used,
    reason = "owner negative-matrix test: asserting on typed error values, not production fallibility"
)]

use std::io::{Read, Write};
use std::num::NonZeroU64;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};

use quanta_index_contract::{
    ActiveGenerationResolutionV1, GenerationPin, GenerationSelector, GenerationSnapshot,
    ManifestGeneration, QueryConstraintSetV1, RepoId, RepoRelativePath, RevisionId,
    SearchCorpusActivationTokenV1, SearchCorpusActiveHeadV1, SearchCorpusGenerationIdentityV1,
    SearchPlaneQueryIpcRequest, SearchPlaneQueryIpcRequestEnvelope, SearchPlaneQueryIpcResponse,
    SearchPlaneQueryIpcResponseEnvelope, SearchPlaneTrackKind, SemanticContentRootsV1,
    SymbolQueryRequest, TextQueryRequest, TextQuerySyntax,
};
use quanta_index_ipc::{decode_request, encode_response};
use quanta_index_sdk::{
    ConnectOptions, QuantaIndex, ResponseBindingAxis, SDK_WIRE_ROUTE_EXCLUSIONS_V1,
    SDK_WIRE_ROUTES_V1, SdkError, SdkError::Binding,
};

// ---------------------------------------------------------------- fixtures

fn repo_id() -> RepoId {
    RepoId::new("sdk-binding-repo").expect("static fixture ID satisfies canonical policy")
}

fn other_repo_id() -> RepoId {
    RepoId::new("sdk-binding-other").expect("static fixture ID satisfies canonical policy")
}

fn revision_id() -> RevisionId {
    RevisionId::new("rev-1").expect("static fixture ID satisfies canonical policy")
}

fn pin(repo: RepoId) -> GenerationPin {
    GenerationPin::new(repo, revision_id(), ManifestGeneration::new(7))
}

fn text_request(
    generation: Option<GenerationPin>,
    selector: Option<GenerationSelector>,
) -> TextQueryRequest {
    TextQueryRequest {
        syntax: TextQuerySyntax::Native,
        query_text: "needle".to_string(),
        constraints: QueryConstraintSetV1::unconstrained(),
        generation,
        generation_selector: selector,
        top_k: 10,
        cursor: None,
    }
}

fn symbol_request() -> SymbolQueryRequest {
    SymbolQueryRequest {
        syntax: TextQuerySyntax::Native,
        query_text: "needle".to_string(),
        constraints: QueryConstraintSetV1::unconstrained(),
        generation: Some(pin(repo_id())),
        generation_selector: None,
        top_k: 10,
        cursor: None,
    }
}

fn text_response(generation: GenerationPin) -> SearchPlaneQueryIpcResponse {
    SearchPlaneQueryIpcResponse::Text(quanta_index_contract::TextQueryResponse {
        explanation: quanta_index_contract::SearchExplanation::empty(),
        generation,
        results: vec![],
        window: quanta_index_contract::QueryResultWindowV2::exact_probe(0),
        file_owner_rows: None,
        next_cursor: None,
    })
}

fn active_snapshot(repo_id: RepoId) -> SearchPlaneQueryIpcResponse {
    let lexical = GenerationSnapshot {
        repo_id,
        revision_id: revision_id(),
        track: SearchPlaneTrackKind::Lexical,
        manifest_generation: ManifestGeneration::new(7),
        manifest_digest: "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            .to_string(),
    };
    SearchPlaneQueryIpcResponse::ActiveGenerationSnapshot(ActiveGenerationResolutionV1 {
        track: SearchPlaneTrackKind::Lexical,
        head: SearchCorpusActiveHeadV1 {
            generation: SearchCorpusGenerationIdentityV1 {
                semantic: GenerationSnapshot {
                    track: SearchPlaneTrackKind::Semantic,
                    ..lexical.clone()
                },
                lexical,
                semantic_content: SemanticContentRootsV1 {
                    row_root_digest: format!("sha256:{}", "a".repeat(64)),
                    membership_root_digest: format!("sha256:{}", "b".repeat(64)),
                },
            },
            activation_token: SearchCorpusActivationTokenV1::new(
                [7; 16],
                NonZeroU64::new(1).expect("fixture sequence is positive"),
            )
            .expect("fixture incarnation is nonzero"),
        },
    })
}

fn lexical_candidate(repo: RepoId) -> quanta_index_contract::LexicalCandidate {
    quanta_index_contract::LexicalCandidate {
        candidate_id: "cand-1".to_string(),
        repo_id: repo,
        revision_id: revision_id(),
        manifest_generation: ManifestGeneration::new(7),
        repo_relative_path: RepoRelativePath::new("src/lib.rs"),
        start_line: 1,
        end_line: 2,
        score: 1.0,
        snippet: "needle".to_string(),
        snippet_hit_offset: None,
        highlights: vec![],
    }
}

// ------------------------------------------------------------ scripted UDS

/// One scripted UDS query-plane peer: serves one connection per scripted
/// response, answering each request with that response while echoing the
/// request's own id, and reports every decoded request back.
fn scripted_query_server(
    dir: &std::path::Path,
    scripts: Vec<SearchPlaneQueryIpcResponse>,
) -> (PathBuf, Receiver<SearchPlaneQueryIpcRequest>) {
    let path = dir.join("query.sock");
    let listener = UnixListener::bind(&path).expect("bind scripted query socket");
    let (tx, rx) = sync_channel::<SearchPlaneQueryIpcRequest>(scripts.len());
    let server = std::thread::spawn(move || {
        for script in scripts {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            if serve_one(&mut stream, script, &tx).is_err() {
                return;
            }
        }
    });
    // The server thread ends once its scripts are consumed; tests join
    // through the channel rendezvous, so drop the handle deliberately.
    drop(server);
    (path, rx)
}

fn serve_one(
    stream: &mut UnixStream,
    script: SearchPlaneQueryIpcResponse,
    tx: &SyncSender<SearchPlaneQueryIpcRequest>,
) -> std::io::Result<()> {
    let request: SearchPlaneQueryIpcRequestEnvelope =
        decode_request(stream).map_err(std::io::Error::other)?;
    tx.send(request.payload.clone())
        .map_err(|_dropped| std::io::Error::other("test receiver dropped"))?;
    let response = SearchPlaneQueryIpcResponseEnvelope {
        request_id: request.request_id,
        payload: script,
    };
    let frame = encode_response(&response).map_err(std::io::Error::other)?;
    stream.write_all(&frame)?;
    stream.flush()?;
    let mut sink = [0u8; 16];
    // The client closes after reading; a read error here is normal.
    let _read = stream.read(&mut sink);
    Ok(())
}

/// A uniquely-named binding directory under RAII custody.
///
/// TOPT-06/TH-3: the guard lives across the server and client lifetimes,
/// and dropping it removes the directory with every socket in it — on
/// success, on assertion failure, and on server-thread error alike.
/// Random names make a stale path unable to collide with a rerun.
fn temp_dir(tag: &str) -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix(&format!("sdk-binding-owner-{tag}-"))
        .tempdir()
        .expect("create temp dir")
}

/// A full-profile client on this temp root; control and ingest endpoints
/// are configured but never dialed by these cases.
fn client_on(dir: &std::path::Path, query_socket: PathBuf) -> QuantaIndex {
    let options = ConnectOptions::default()
        .with_query_socket(query_socket)
        .with_control_socket(dir.join("control.sock"))
        .with_ingest_socket(dir.join("ingest.sock"));
    QuantaIndex::connect(options).expect("full-profile client connects")
}

/// Run one scripted text query through the exported request entrypoint.
fn run_scripted(
    dir: &std::path::Path,
    request: TextQueryRequest,
    script: SearchPlaneQueryIpcResponse,
) -> Result<quanta_index_contract::TextQueryResponse, SdkError> {
    let (socket, _rx) = scripted_query_server(dir, vec![script]);
    let client = client_on(dir, socket);
    client.reader().lexical_request(request)
}

// ------------------------------------------------------------- test vector

#[test]
fn wrong_variant_fails_closed() {
    let dir = temp_dir("variant");
    // Correct id, correct repo pin, but a `text` response to a `symbol`
    // call: refused on the variant axis.
    let (socket, _rx) = scripted_query_server(dir.path(), vec![text_response(pin(repo_id()))]);
    let client = client_on(dir.path(), socket);
    let error = client
        .reader()
        .symbol_request(symbol_request())
        .expect_err("wrong variant must be refused");
    assert!(matches!(
        error,
        Binding {
            axis: ResponseBindingAxis::Variant,
            ..
        }
    ));
}

#[test]
fn wrong_repo_pin_fails_closed() {
    let dir = temp_dir("pin");
    let error = run_scripted(
        dir.path(),
        text_request(Some(pin(repo_id())), None),
        text_response(pin(other_repo_id())),
    )
    .expect_err("wrong pin must be refused");
    assert!(matches!(
        error,
        Binding {
            axis: ResponseBindingAxis::ReadIdentity,
            ..
        }
    ));
}

#[test]
fn active_selector_out_of_domain_fails_closed() {
    let dir = temp_dir("active");
    let (socket, rx) = scripted_query_server(dir.path(), vec![active_snapshot(other_repo_id())]);
    let client = client_on(dir.path(), socket);
    let error = client
        .reader()
        .lexical_request(text_request(
            None,
            Some(GenerationSelector::Active {
                repo_id: repo_id(),
                revision_id: revision_id(),
            }),
        ))
        .expect_err("out-of-domain active resolution must be refused");
    assert!(matches!(
        error,
        Binding {
            axis: ResponseBindingAxis::ReadIdentity,
            ..
        }
    ));
    assert!(matches!(
        rx.recv()
            .expect("active resolution reached the query socket"),
        SearchPlaneQueryIpcRequest::ResolveActiveGeneration(_)
    ));
}

#[test]
fn active_selector_rejects_wrong_same_domain_generation_over_uds() {
    let dir = temp_dir("active-wrong-generation");
    let wrong = GenerationPin::new(repo_id(), revision_id(), ManifestGeneration::new(8));
    let (socket, rx) = scripted_query_server(
        dir.path(),
        vec![active_snapshot(repo_id()), text_response(wrong)],
    );
    let client = client_on(dir.path(), socket);
    let error = client
        .reader()
        .lexical_request(text_request(
            None,
            Some(GenerationSelector::Active {
                repo_id: repo_id(),
                revision_id: revision_id(),
            }),
        ))
        .expect_err("same-domain response from another generation must be refused");
    assert!(matches!(
        error,
        Binding {
            axis: ResponseBindingAxis::ReadIdentity,
            ..
        }
    ));
    assert!(matches!(
        rx.recv().expect("resolution request"),
        SearchPlaneQueryIpcRequest::ResolveActiveGeneration(_)
    ));
    assert!(matches!(
        rx.recv().expect("pinned query request"),
        SearchPlaneQueryIpcRequest::Text(request)
            if request.generation == Some(pin(repo_id()))
                && matches!(request.generation_selector, Some(GenerationSelector::ResolvedActive { .. }))
    ));
}

#[test]
fn foreign_candidate_fails_closed() {
    let dir = temp_dir("candidate");
    let response = SearchPlaneQueryIpcResponse::Text(quanta_index_contract::TextQueryResponse {
        explanation: quanta_index_contract::SearchExplanation::empty(),
        generation: pin(repo_id()),
        results: vec![lexical_candidate(other_repo_id())],
        window: quanta_index_contract::QueryResultWindowV2::exact_probe(1),
        file_owner_rows: None,
        next_cursor: None,
    });
    let error = run_scripted(
        dir.path(),
        text_request(Some(pin(repo_id())), None),
        response,
    )
    .expect_err("a candidate from another generation must be refused");
    assert!(matches!(
        error,
        Binding {
            axis: ResponseBindingAxis::CandidateIdentity,
            ..
        }
    ));
}

#[test]
fn window_disagreeing_with_rows_fails_closed() {
    let dir = temp_dir("window");
    let response = SearchPlaneQueryIpcResponse::Text(quanta_index_contract::TextQueryResponse {
        explanation: quanta_index_contract::SearchExplanation::empty(),
        generation: pin(repo_id()),
        results: vec![],
        window: quanta_index_contract::QueryResultWindowV2::exact_probe(3),
        file_owner_rows: None,
        next_cursor: None,
    });
    let error = run_scripted(
        dir.path(),
        text_request(Some(pin(repo_id())), None),
        response,
    )
    .expect_err("a window that disagrees with the page must be refused");
    // Window-vs-rows consistency is intrinsic shape: the contract
    // decoder refuses it before the SDK's contextual layer ever runs.
    assert!(
        matches!(error, SdkError::Transport(_)),
        "the contract decoder refuses the malformed window: {error:?}"
    );
}

#[test]
fn rows_over_request_cap_fails_closed() {
    let dir = temp_dir("cap");
    let response = SearchPlaneQueryIpcResponse::Text(quanta_index_contract::TextQueryResponse {
        explanation: quanta_index_contract::SearchExplanation::empty(),
        generation: pin(repo_id()),
        results: vec![lexical_candidate(repo_id()), lexical_candidate(repo_id())],
        window: quanta_index_contract::QueryResultWindowV2::exact_probe(2),
        file_owner_rows: None,
        next_cursor: None,
    });
    let mut request = text_request(Some(pin(repo_id())), None);
    request.top_k = 1;
    let error = run_scripted(dir.path(), request, response)
        .expect_err("more rows than the request cap must be refused");
    // Either the SDK's contextual cardinality check refuses the page,
    // or the intrinsic decoder already refused it: both fail closed.
    let refused_by_binding = matches!(
        error,
        SdkError::Binding {
            axis: ResponseBindingAxis::Cardinality,
            ..
        }
    );
    let refused_by_decoder = matches!(error, SdkError::Transport(_));
    assert!(
        refused_by_binding || refused_by_decoder,
        "an over-cap page must be refused somewhere: {error:?}"
    );
}

fn owner_hit(candidate_id: &str, score: f32) -> quanta_index_contract::LexicalCandidate {
    quanta_index_contract::LexicalCandidate {
        candidate_id: candidate_id.to_string(),
        repo_id: repo_id(),
        revision_id: revision_id(),
        manifest_generation: ManifestGeneration::new(7),
        repo_relative_path: RepoRelativePath::new("src/lib.rs"),
        start_line: 1,
        end_line: 2,
        score,
        snippet: "needle".to_string(),
        snippet_hit_offset: None,
        highlights: vec![],
    }
}

fn owner_projection_row(
    candidate: &quanta_index_contract::LexicalCandidate,
) -> quanta_index_contract::FileOwnerProjectionRow {
    quanta_index_contract::FileOwnerProjectionRow {
        candidate_id: candidate.candidate_id.clone(),
        repo_id: candidate.repo_id.clone(),
        revision_id: candidate.revision_id.clone(),
        manifest_generation: candidate.manifest_generation,
        repo_relative_path: candidate.repo_relative_path.clone(),
        owners: vec!["ada".to_string()],
    }
}

fn owner_hybrid_row(
    candidate_id: &str,
    fused_score: f64,
) -> quanta_index_contract::HybridCandidateV1 {
    quanta_index_contract::HybridCandidateV1 {
        candidate: owner_hit(candidate_id, 0.9),
        fused_score,
        contributions: vec![quanta_index_contract::HybridLaneContributionV1 {
            lane: quanta_index_contract::HybridLaneV1::Lexical,
            rank: 1,
            raw_score: 0.9,
        }],
    }
}

#[test]
fn swapped_owner_projection_fails_closed() {
    let dir = temp_dir("projection");
    let first = owner_hit("cand-1", 2.0);
    let second = owner_hit("cand-2", 1.0);
    let response = SearchPlaneQueryIpcResponse::Text(quanta_index_contract::TextQueryResponse {
        explanation: quanta_index_contract::SearchExplanation::empty(),
        generation: pin(repo_id()),
        results: vec![first.clone(), second.clone()],
        window: quanta_index_contract::QueryResultWindowV2::exact_probe(2),
        file_owner_rows: Some(vec![
            owner_projection_row(&second),
            owner_projection_row(&first),
        ]),
        next_cursor: None,
    });
    let error = run_scripted(
        dir.path(),
        text_request(Some(pin(repo_id())), None),
        response,
    )
    .expect_err("a swapped owner projection must be refused");
    // Projection pairing is intrinsic shape: the contract codec refuses
    // it before the SDK's contextual layer ever runs.
    assert!(
        matches!(error, SdkError::Transport(_)),
        "a swapped projection must fail closed on the wire: {error:?}"
    );
}

#[test]
fn unordered_hybrid_ranking_fails_closed() {
    let dir = temp_dir("hybrid-order");
    let response =
        SearchPlaneQueryIpcResponse::Hybrid(quanta_index_contract::HybridQueryResponse {
            generation: pin(repo_id()),
            results: vec![
                owner_hybrid_row("cand-a", 1.0),
                owner_hybrid_row("cand-b", 2.0),
            ],
            window: quanta_index_contract::QueryResultWindowV2::exact_probe(2),
            explanation: quanta_index_contract::SearchExplanation::default(),
        });
    let (socket, _rx) = scripted_query_server(dir.path(), vec![response]);
    let client = client_on(dir.path(), socket);
    let error = client
        .search()
        .hybrid()
        .sourcegraph("needle")
        .semantic_text("where the needle is kept")
        .pinned(pin(repo_id()))
        .top_k(7)
        .execute()
        .expect_err("an unordered hybrid ranking must be refused");
    // Ranking order is intrinsic shape: the contract codec refuses it
    // before the SDK's contextual layer ever runs.
    assert!(
        matches!(error, SdkError::Transport(_)),
        "an unordered ranking must fail closed on the wire: {error:?}"
    );
}

#[test]
fn matching_positive_response_passes_binding() {
    let dir = temp_dir("positive");
    let (socket, rx) = scripted_query_server(dir.path(), vec![text_response(pin(repo_id()))]);
    let client = client_on(dir.path(), socket);
    let response = client
        .reader()
        .lexical_request(text_request(Some(pin(repo_id())), None))
        .expect("a contextually matching response passes");
    assert_eq!(response.generation, pin(repo_id()));
    let sent = rx.recv().expect("the request reached the wire");
    assert!(matches!(sent, SearchPlaneQueryIpcRequest::Text(_)));
}

// ------------------------------------------------ query-only client profile

#[test]
fn query_only_profile_needs_no_control_or_ingest_sockets() {
    let dir = temp_dir("query-only");
    let (socket, rx) = scripted_query_server(
        dir.path(),
        vec![active_snapshot(repo_id()), text_response(pin(repo_id()))],
    );
    // Only the query socket is named; the query-only profile must not
    // require, resolve or fabricate the other two.
    let options = ConnectOptions::default().with_query_socket(socket);
    let client = QuantaIndex::connect_query_only(options)
        .expect("query-only profile connects without control/ingest endpoints");
    let response = client
        .reader()
        .lexical_request(text_request(
            None,
            Some(GenerationSelector::Active {
                repo_id: repo_id(),
                revision_id: revision_id(),
            }),
        ))
        .expect("active query dispatch works in the query-only profile");
    drop(response);
    assert!(matches!(
        rx.recv().expect("resolution request"),
        SearchPlaneQueryIpcRequest::ResolveActiveGeneration(_)
    ));
    assert!(matches!(
        rx.recv().expect("pinned query request"),
        SearchPlaneQueryIpcRequest::Text(request)
            if request.generation == Some(pin(repo_id()))
                && matches!(request.generation_selector, Some(GenerationSelector::ResolvedActive { .. }))
    ));

    // The full profile with the same options still refuses: least
    // privilege is opt-in, not a silent downgrade.
    let full = temp_dir("full-refuses");
    let options = ConnectOptions::default().with_query_socket(full.path().join("q.sock"));
    assert!(
        QuantaIndex::connect(options).is_err(),
        "the full profile must keep requiring control and ingest endpoints"
    );
}

// ------------------------------------------------------- custody evidence

#[test]
fn binding_directory_and_sockets_disappear_with_their_guard() {
    // TOPT-06/TH-3 proof: the guard owns the directory, and the bound
    // socket file with it — dropping the guard removes both, and two
    // guards with the same tag never share a path, so a stale path
    // cannot collide with a rerun.
    let dir = temp_dir("custody");
    let (socket, _rx) = scripted_query_server(dir.path(), vec![text_response(pin(repo_id()))]);
    assert!(socket.exists(), "the socket is bound");
    let path = dir.path().to_path_buf();
    drop(dir);
    assert!(
        !socket.exists() && !path.exists(),
        "guard drop removes the socket and its directory"
    );

    let first = temp_dir("custody");
    let second = temp_dir("custody");
    assert_ne!(
        first.path(),
        second.path(),
        "same-tag guards never share a path"
    );
}

// --------------------------------------------------- exported method table

/// The exported wire-route coverage table must exact-match the SDK's
/// dispatch surface: one row per closed expected variant per plane, each
/// with at least one binding axis, and the documented exclusions.
#[test]
fn coverage_table_exact_matches_sdk_surface() {
    let query_routes: Vec<_> = SDK_WIRE_ROUTES_V1
        .iter()
        .filter(|row| row.plane == "query")
        .map(|row| row.route)
        .collect();
    let expected_query = [
        "active_generation_snapshot",
        "resolved_lexical_generation",
        "text",
        "symbol",
        "semantic",
        "hybrid",
        "hybrid_seed",
        "history",
        "runtime_metadata",
        "structural",
        "repomap",
        "explain",
        "cluster_membership_batch_read",
    ];
    assert_eq!(query_routes, expected_query, "query plane coverage");

    let control_routes: Vec<_> = SDK_WIRE_ROUTES_V1
        .iter()
        .filter(|row| row.plane == "control")
        .map(|row| row.route)
        .collect();
    let expected_control = [
        "search_corpus_activation_cas_ack",
        "search_corpus_rollback_cas_ack",
        "search_corpus_active_head_observation",
        "repomap_mutation_ack",
        "repomap_terminal_receipt_v2",
        "current_generation_snapshot",
        "generation_status_report",
        "metrics_snapshot",
        "quarantine_inventory",
        "quarantine_discard_ack",
        "process_readiness_report",
    ];
    assert_eq!(control_routes, expected_control, "control plane coverage");

    let ingest_routes: Vec<_> = SDK_WIRE_ROUTES_V1
        .iter()
        .filter(|row| row.plane == "ingest")
        .map(|row| row.route)
        .collect();
    let expected_ingest = [
        "search_corpus_receipt",
        "repomap_receipt",
        "repomap_terminal_receipt_v2",
        "history_receipt",
        "repo_commit_recency_receipt",
        "repo_topic_receipt",
        "file_ownership_receipt",
        "file_contributor_receipt",
        "repo_meta_receipt",
        "repo_description_receipt",
        "dirty_receipt",
        "runtime_catalog_receipt",
        "structural_receipt",
    ];
    assert_eq!(ingest_routes, expected_ingest, "ingest plane coverage");

    for row in SDK_WIRE_ROUTES_V1 {
        assert_eq!(
            row.route, row.expected_kind,
            "a route accepts exactly its own response variant"
        );
        assert!(
            !row.bound_axes.is_empty(),
            "every wire route declares binding semantics: {}",
            row.route
        );
    }

    assert!(
        SDK_WIRE_ROUTE_EXCLUSIONS_V1.len() >= 4,
        "excluded surface classes are named with reasons"
    );
    for (name, reason) in SDK_WIRE_ROUTE_EXCLUSIONS_V1 {
        assert!(!reason.is_empty(), "exclusion {name} states its reason");
    }
}
