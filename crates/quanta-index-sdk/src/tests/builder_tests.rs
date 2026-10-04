use super::*;

// QI-BB-025: the builder refuses an out-of-range `top_k` locally, under the same
// code the daemon answers with, and never puts the request on the wire.
#[test]
fn text_query_builder_refuses_out_of_range_top_k_before_any_round_trip() {
    for top_k in [0, quanta_index_contract::PUBLIC_TOP_K_MAX + 1, u32::MAX] {
        let query = unused_query();
        let client = QuantaIndex::from_transports(query.clone(), unused_control(), unused_ingest());
        let outcome = client
            .lexical()
            .query()
            .native("needle")
            .active(
                RepoId::new("repo").expect("static fixture ID satisfies canonical policy"),
                RevisionId::new("rev").expect("static fixture ID satisfies canonical policy"),
            )
            .top_k(top_k)
            .execute();
        match outcome {
            Err(crate::SdkError::Remote { code, .. }) => assert_eq!(
                code.as_wire_str(),
                quanta_index_contract::TOP_K_OUT_OF_RANGE_CODE,
                "top_k={top_k}"
            ),
            other => panic!("top_k={top_k} must be refused with the shared code, got {other:?}"),
        }
        let sent = ok_or_fail!(query.requests.lock()).len();
        assert_eq!(sent, 0, "a refused top_k must not reach the transport");
    }
}

#[test]
fn text_query_builder_accepts_the_public_maximum_top_k() {
    let query = Arc::new(StubQueryTransport::active(
        SearchPlaneQueryIpcResponse::Text(TextQueryResponse {
            selected_active_head: None,
            rank_unit: quanta_index_contract::TextRankUnit::Chunk,
            explanation: quanta_index_contract::SearchExplanation::empty(),
            generation: sample_generation_pin(),
            results: vec![],
            window: QueryResultWindowV2::exact_probe(0),
            file_owner_rows: None,
            next_cursor: None,
        }),
    ));
    let client = QuantaIndex::from_transports(query.clone(), unused_control(), unused_ingest());
    let _response = ok_or_fail!(
        client
            .lexical()
            .query()
            .native("needle")
            .active(repo_id(), revision_id())
            .top_k(quanta_index_contract::PUBLIC_TOP_K_MAX)
            .execute()
    );
    let captured = ok_or_fail!(only_query_request(query.as_ref()));
    let quanta_index_contract::SearchPlaneQueryIpcRequest::Text(request) = captured.payload else {
        panic!(
            "expected a text query on the wire, got {:?}",
            captured.payload
        );
    };
    assert_eq!(request.top_k, quanta_index_contract::PUBLIC_TOP_K_MAX);
}

// QI-BB-025: the hybrid-seed builder has its own request assembly path and
// must apply the same local gate as the text builders — it is the one route
// whose SDK builder does not go through `build_request`.
#[test]
fn hybrid_seed_builder_refuses_out_of_range_top_k_before_any_round_trip() {
    for top_k in [0, quanta_index_contract::PUBLIC_TOP_K_MAX + 1, u32::MAX] {
        let query = unused_query();
        let client = QuantaIndex::from_transports(query.clone(), unused_control(), unused_ingest());
        let outcome = client
            .search()
            .hybrid_seed()
            .native("needle")
            .semantic_text("needle")
            .active(repo_id(), revision_id())
            .top_k(top_k)
            .execute();
        match outcome {
            Err(crate::SdkError::Remote { code, .. }) => assert_eq!(
                code.as_wire_str(),
                quanta_index_contract::TOP_K_OUT_OF_RANGE_CODE,
                "top_k={top_k}"
            ),
            other => panic!("top_k={top_k} must be refused with the shared code, got {other:?}"),
        }
        let sent = ok_or_fail!(query.requests.lock()).len();
        assert_eq!(sent, 0, "a refused top_k must not reach the transport");
    }
}

/// The runtime builder's `after(cursor)` carries the cursor a previous
/// page returned onto the wire untouched (QI-BB-025 W4); without it the
/// request carries no cursor.
#[test]
fn runtime_query_builder_carries_the_cursor_it_continues_from() {
    let cursor = ok_or_fail!(ContinuationTokenV2::new("signed-runtime-page"));
    let query = Arc::new(StubQueryTransport::new(
        SearchPlaneQueryIpcResponse::RuntimeMetadata(SearchPlaneRuntimeMetadataQueryResponse {
            selected_active_head: None,
            generation: sample_generation_pin(),
            results: vec![],
            window: QueryResultWindowV2::exact_probe(0),
            read_epoch: quanta_index_contract::AuxEpochV1::new(4),
            universe_epoch: quanta_index_contract::AuxEpochV1::new(9),
            examined: 0,
            next_cursor: None,
        }),
    ));
    let client = QuantaIndex::from_transports(query.clone(), unused_control(), unused_ingest());
    let _response = ok_or_fail!(
        client
            .runtime()
            .query()
            .sourcegraph("dirty:yes")
            .pinned(sample_generation_pin())
            .top_k(3)
            .after(cursor.clone())
            .execute()
    );
    let captured = ok_or_fail!(only_query_request(query.as_ref()));
    let quanta_index_contract::SearchPlaneQueryIpcRequest::RuntimeMetadata(req) = &captured.payload
    else {
        panic!(
            "expected RuntimeMetadata request, got {:?}",
            captured.payload
        );
    };
    assert_eq!(req.cursor.as_ref(), Some(&cursor));

    // A builder that never called `after` walks fresh.
    let fresh_query = Arc::new(StubQueryTransport::new(
        SearchPlaneQueryIpcResponse::RuntimeMetadata(SearchPlaneRuntimeMetadataQueryResponse {
            selected_active_head: None,
            generation: sample_generation_pin(),
            results: vec![],
            window: QueryResultWindowV2::exact_probe(0),
            read_epoch: quanta_index_contract::AuxEpochV1::new(4),
            universe_epoch: quanta_index_contract::AuxEpochV1::new(9),
            examined: 0,
            next_cursor: None,
        }),
    ));
    let client =
        QuantaIndex::from_transports(fresh_query.clone(), unused_control(), unused_ingest());
    let _response = ok_or_fail!(
        client
            .runtime()
            .query()
            .sourcegraph("dirty:yes")
            .pinned(sample_generation_pin())
            .top_k(3)
            .execute()
    );
    let captured = ok_or_fail!(only_query_request(fresh_query.as_ref()));
    let quanta_index_contract::SearchPlaneQueryIpcRequest::RuntimeMetadata(fresh) =
        &captured.payload
    else {
        panic!(
            "expected RuntimeMetadata request, got {:?}",
            captured.payload
        );
    };
    assert_eq!(fresh.cursor, None);
}

/// The structural builder's `after(cursor)` carries the cursor a previous
/// page returned onto the wire untouched (QI-BB-025 W4).
#[test]
fn structural_query_builder_carries_the_cursor_it_continues_from() {
    let cursor = ok_or_fail!(ContinuationTokenV2::new("signed-structural-page"));
    let query = Arc::new(StubQueryTransport::new(
        SearchPlaneQueryIpcResponse::Structural(SearchPlaneStructuralQueryResponse {
            generation: sample_generation_pin(),
            results: vec![],
            window: QueryResultWindowV2::exact_probe(0),
            read_epoch: quanta_index_contract::AuxEpochV1::new(6),
            examined: 0,
            next_cursor: None,
        }),
    ));
    let client = QuantaIndex::from_transports(query.clone(), unused_control(), unused_ingest());
    let _response = ok_or_fail!(
        client
            .structural()
            .query()
            .native("match { :[x] }")
            .pinned(sample_generation_pin())
            .top_k(2)
            .after(cursor.clone())
            .execute()
    );
    let captured = ok_or_fail!(only_query_request(query.as_ref()));
    let quanta_index_contract::SearchPlaneQueryIpcRequest::Structural(req) = &captured.payload
    else {
        panic!("expected Structural request, got {:?}", captured.payload);
    };
    assert_eq!(req.cursor.as_ref(), Some(&cursor));
}

/// QI-BB-018: the true-hybrid route is reachable from the SDK.
///
/// The builder assembles one `Hybrid` request with the text lane, the
/// dense text, the selection and the fused `top_k` on both the outer
/// request and the text lane, under the pushed-down constraints.
#[test]
fn hybrid_builder_assembles_a_hybrid_request_with_both_lanes() {
    let rust = ok_or_fail!(LanguageCode::new("rust"));
    let path = ok_or_fail!(ExactRepoRelativePathV1::new("src/lib.rs"));
    let query = Arc::new(StubQueryTransport::new(
        SearchPlaneQueryIpcResponse::Hybrid(quanta_index_contract::HybridQueryResponse {
            selected_active_head: None,
            generation: sample_generation_pin(),
            results: Vec::new(),
            window: QueryResultWindowV2::exact_probe(0),
            explanation: sample_explanation(),
        }),
    ));
    let client = QuantaIndex::from_transports(query.clone(), unused_control(), unused_ingest());
    let response = ok_or_fail!(
        client
            .search()
            .hybrid()
            .sourcegraph("needle")
            .semantic_text("where the needle is kept")
            .language_any_of([rust.clone()])
            .exact_repo_relative_path(path.clone())
            .pinned(sample_generation_pin())
            .top_k(7)
            .execute()
    );
    assert_eq!(response.generation, sample_generation_pin());
    let captured = ok_or_fail!(only_query_request(query.as_ref()));
    let quanta_index_contract::SearchPlaneQueryIpcRequest::Hybrid(request) = &captured.payload
    else {
        panic!("expected Hybrid request, got {:?}", captured.payload);
    };
    assert_eq!(request.top_k, 7);
    assert_eq!(request.text_query.top_k, 7);
    assert_eq!(
        request.text_query.syntax,
        quanta_index_contract::TextQuerySyntax::Sourcegraph
    );
    assert_eq!(request.text_query.query_text, "needle");
    assert_eq!(request.semantic_query_text, "where the needle is kept");
    assert_eq!(request.generation, Some(sample_generation_pin()));
    assert_eq!(request.generation_selector, None);
    assert_eq!(
        request
            .text_query
            .constraints
            .repo_relative_path_exact
            .as_ref(),
        Some(&path)
    );
    assert!(
        request
            .text_query
            .constraints
            .language_any_of
            .contains(&rust)
    );
}

/// QI-BB-025: the hybrid builder applies the shared `top_k` gate before any
/// round trip, under the one code every route reports.
#[test]
fn hybrid_builder_refuses_out_of_range_top_k_before_any_round_trip() {
    for top_k in [0, quanta_index_contract::PUBLIC_TOP_K_MAX + 1, u32::MAX] {
        let query = unused_query();
        let client = QuantaIndex::from_transports(query.clone(), unused_control(), unused_ingest());
        let outcome = client
            .search()
            .hybrid()
            .native("needle")
            .semantic_text("needle")
            .active(repo_id(), revision_id())
            .top_k(top_k)
            .execute();
        match outcome {
            Err(crate::SdkError::Remote { code, .. }) => assert_eq!(
                code.as_wire_str(),
                quanta_index_contract::TOP_K_OUT_OF_RANGE_CODE,
                "top_k={top_k}"
            ),
            other => panic!("top_k={top_k} must be refused with the shared code, got {other:?}"),
        }
        let sent = ok_or_fail!(query.requests.lock()).len();
        assert_eq!(sent, 0, "a refused top_k must not reach the transport");
    }
}

/// QI-BB-022: a hybrid row is explained under both its queries; the SDK
/// sends the row, the text query at the fused `top_k`, and the dense text.
#[test]
fn hybrid_explain_carries_the_row_and_both_queries() {
    let row = quanta_index_contract::HybridCandidateV1 {
        candidate: quanta_index_contract::LexicalCandidate {
            source_repo_id: repo_id(),
            source: None,
            preview: None,
            candidate_id: "chunk://alpha".to_string(),
            repo_id: repo_id(),
            revision_id: revision_id(),
            manifest_generation: ManifestGeneration::new(7),
            repo_relative_path: RepoRelativePath::new("src/lib.rs"),
            start_line: 1,
            end_line: 2,
            score: 1.5,
            snippet: "needle".to_string(),
            snippet_hit_offset: None,
            highlights: Vec::new(),
        },
        fused_score: 1.0 / 61.0 + 1.0 / 62.0,
        contributions: vec![
            quanta_index_contract::HybridLaneContributionV1 {
                lane: quanta_index_contract::HybridLaneV1::Lexical,
                rank: 1,
                raw_score: 1.5,
            },
            quanta_index_contract::HybridLaneContributionV1 {
                lane: quanta_index_contract::HybridLaneV1::Dense,
                rank: 2,
                raw_score: 0.5,
            },
        ],
    };
    let query = Arc::new(StubQueryTransport::new(
        SearchPlaneQueryIpcResponse::Explain(
            quanta_index_contract::SearchPlaneExplainQueryResponse {
                generation: sample_generation_pin(),
                presence: quanta_index_contract::CandidatePresenceV1::Indexed,
                explanation: sample_explanation(),
            },
        ),
    ));
    let client = QuantaIndex::from_transports(query.clone(), unused_control(), unused_ingest());
    let text_query = quanta_index_contract::TextQueryRequest {
        syntax: quanta_index_contract::TextQuerySyntax::Sourcegraph,
        query_text: "needle".to_string(),
        constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
        generation: None,
        generation_selector: None,
        top_k: 7,
        cursor: None,
    };
    let response = ok_or_fail!(client.search().explain_hybrid_under_queries(
        sample_generation_pin(),
        row.clone(),
        text_query.clone(),
        "where the needle is kept",
    ));
    assert_eq!(
        response.presence,
        quanta_index_contract::CandidatePresenceV1::Indexed
    );
    let captured = ok_or_fail!(only_query_request(query.as_ref()));
    let quanta_index_contract::SearchPlaneQueryIpcRequest::Explain(request) = &captured.payload
    else {
        panic!("expected Explain request, got {:?}", captured.payload);
    };
    assert_eq!(
        request.candidate,
        quanta_index_contract::ExplainCandidateV1::Hybrid(row)
    );
    assert_eq!(request.text_query.as_ref(), Some(&text_query));
    assert_eq!(
        request.semantic_query_text.as_deref(),
        Some("where the needle is kept")
    );
}

fn binding_hit(candidate_id: &str, score: f32) -> quanta_index_contract::LexicalCandidate {
    quanta_index_contract::LexicalCandidate {
        source_repo_id: repo_id(),
        source: None,
        preview: None,
        candidate_id: candidate_id.to_string(),
        repo_id: repo_id(),
        revision_id: revision_id(),
        manifest_generation: ManifestGeneration::new(7),
        repo_relative_path: RepoRelativePath::new("src/lib.rs"),
        start_line: 10,
        end_line: 20,
        score,
        snippet: "fn sample() {}".to_string(),
        snippet_hit_offset: None,
        highlights: Vec::new(),
    }
}

#[test]
fn typed_code_search_response_rejects_mismatched_source_path() {
    use quanta_index_contract::{
        PreviewKind, PreviewMetadata, QueryConstraintSetV1, SourceFileKey, SourceFileRevision,
        TextQueryRequest, TextQuerySyntax, TextRankUnit,
    };

    let request = quanta_index_contract::SearchPlaneQueryIpcRequest::Text(TextQueryRequest {
        syntax: TextQuerySyntax::CodeSearch,
        query_text: "needle".to_string(),
        constraints: QueryConstraintSetV1::unconstrained(),
        generation: Some(sample_generation_pin()),
        generation_selector: None,
        top_k: 10,
        cursor: None,
    });
    let binding = crate::binding::QueryCallBinding::from_request(&request);
    let mut row = binding_hit("file:fixture", 1.0);
    let source = SourceFileRevision {
        file: SourceFileKey {
            source_repo_id: row.source_repo_id.clone(),
            repo_relative_path: RepoRelativePath::new("src/other.rs"),
        },
        revision_id: revision_id(),
        source_sha256: [7; 32],
    };
    row.source = Some(source.clone());
    row.preview = Some(PreviewMetadata {
        kind: PreviewKind::Path,
        source: Some(source),
        chunk_start_byte: None,
        original_focus: None,
        original_context: None,
        normalized_focus: None,
        normalization_equivalent: false,
        unavailable_reason: None,
    });
    row.snippet = "src/other.rs".to_string();
    let response = SearchPlaneQueryIpcResponse::Text(TextQueryResponse {
        selected_active_head: None,
        generation: sample_generation_pin(),
        rank_unit: TextRankUnit::File,
        results: vec![row],
        window: QueryResultWindowV2::exact_probe(1),
        explanation: SearchExplanation::empty(),
        file_owner_rows: None,
        next_cursor: None,
    });
    assert!(matches!(
        crate::binding::bind_query_response(&binding, &response),
        Err(crate::SdkError::Binding {
            axis: crate::ResponseBindingAxis::CandidateIdentity,
            ..
        })
    ));
}

fn binding_owner_row(
    candidate: &quanta_index_contract::LexicalCandidate,
) -> quanta_index_contract::FileOwnerProjectionRow {
    quanta_index_contract::FileOwnerProjectionRow {
        source_repo_id: candidate.source_repo_id.clone(),
        candidate_id: candidate.candidate_id.clone(),
        repo_id: candidate.repo_id.clone(),
        revision_id: candidate.revision_id.clone(),
        manifest_generation: candidate.manifest_generation,
        repo_relative_path: candidate.repo_relative_path.clone(),
        owners: vec!["ada".to_string()],
    }
}

fn binding_hybrid_row(
    candidate_id: &str,
    fused_score: f64,
) -> quanta_index_contract::HybridCandidateV1 {
    quanta_index_contract::HybridCandidateV1 {
        candidate: binding_hit(candidate_id, 0.9),
        fused_score,
        contributions: vec![quanta_index_contract::HybridLaneContributionV1 {
            lane: quanta_index_contract::HybridLaneV1::Lexical,
            rank: 1,
            raw_score: 0.9,
        }],
    }
}

fn execute_text_with(
    response: SearchPlaneQueryIpcResponse,
) -> Result<TextQueryResponse, crate::SdkError> {
    let query = Arc::new(StubQueryTransport::new(response));
    let client = QuantaIndex::from_transports(query, unused_control(), unused_ingest());
    client
        .lexical()
        .query()
        .native("needle")
        .pinned(sample_generation_pin())
        .top_k(7)
        .execute()
}

/// S21-07: a swapped owner projection is refused on the projection
/// axis even though the variant, pin, window and cap all match.
#[test]
fn text_swapped_owner_projection_is_refused_on_the_projection_axis() {
    let first = binding_hit("cand-1", 2.0);
    let second = binding_hit("cand-2", 1.0);
    let response = SearchPlaneQueryIpcResponse::Text(TextQueryResponse {
        selected_active_head: None,
        rank_unit: quanta_index_contract::TextRankUnit::Chunk,
        explanation: quanta_index_contract::SearchExplanation::empty(),
        generation: sample_generation_pin(),
        results: vec![first.clone(), second.clone()],
        window: QueryResultWindowV2::exact_probe(2),
        file_owner_rows: Some(vec![binding_owner_row(&second), binding_owner_row(&first)]),
        next_cursor: None,
    });
    let error = execute_text_with(response).expect_err("a swapped projection must be refused");
    assert!(
        matches!(
            error,
            crate::SdkError::Binding {
                axis: crate::ResponseBindingAxis::ProjectionPairing,
                ..
            }
        ),
        "expected projection-pairing refusal, got {error:?}"
    );
}

/// S21-07: an exactly paired projection passes binding.
#[test]
fn text_exact_owner_projection_passes_binding() {
    let first = binding_hit("cand-1", 2.0);
    let second = binding_hit("cand-2", 1.0);
    let response = SearchPlaneQueryIpcResponse::Text(TextQueryResponse {
        selected_active_head: None,
        rank_unit: quanta_index_contract::TextRankUnit::Chunk,
        explanation: quanta_index_contract::SearchExplanation::empty(),
        generation: sample_generation_pin(),
        results: vec![first.clone(), second.clone()],
        window: QueryResultWindowV2::exact_probe(2),
        file_owner_rows: Some(vec![binding_owner_row(&first), binding_owner_row(&second)]),
        next_cursor: None,
    });
    let page = ok_or_fail!(execute_text_with(response));
    assert_eq!(page.results.len(), 2);
}

fn execute_hybrid_with(
    results: Vec<quanta_index_contract::HybridCandidateV1>,
) -> Result<quanta_index_contract::HybridQueryResponse, crate::SdkError> {
    let query = Arc::new(StubQueryTransport::new(
        SearchPlaneQueryIpcResponse::Hybrid(quanta_index_contract::HybridQueryResponse {
            selected_active_head: None,
            generation: sample_generation_pin(),
            results,
            window: QueryResultWindowV2::exact_probe(2),
            explanation: sample_explanation(),
        }),
    ));
    let client = QuantaIndex::from_transports(query, unused_control(), unused_ingest());
    client
        .search()
        .hybrid()
        .sourcegraph("needle")
        .semantic_text("where the needle is kept")
        .pinned(sample_generation_pin())
        .top_k(7)
        .execute()
}

/// S21-07: typed transports skip the wire decoder, so binding holds
/// the ranking line itself: rows outside fused-score order are refused.
#[test]
fn hybrid_ranking_outside_fused_score_order_is_refused_on_the_ranking_axis() {
    let error = execute_hybrid_with(vec![
        binding_hybrid_row("cand-a", 1.0),
        binding_hybrid_row("cand-b", 2.0),
    ])
    .expect_err("an unordered ranking must be refused");
    assert!(
        matches!(
            error,
            crate::SdkError::Binding {
                axis: crate::ResponseBindingAxis::RankingOrder,
                ..
            }
        ),
        "expected ranking-order refusal, got {error:?}"
    );
}

/// S21-07: a repeated candidate identity is refused, and the refusal
/// names the kind only — never the repeated identity itself.
#[test]
fn hybrid_duplicate_identity_is_refused_without_payload_leakage() {
    let error = execute_hybrid_with(vec![
        binding_hybrid_row("cand-dup", 2.0),
        binding_hybrid_row("cand-dup", 1.0),
    ])
    .expect_err("a duplicated identity must be refused");
    assert!(
        matches!(
            error,
            crate::SdkError::Binding {
                axis: crate::ResponseBindingAxis::RankingOrder,
                ..
            }
        ),
        "expected ranking-order refusal, got {error:?}"
    );
    let rendered = format!("{error}");
    assert!(
        !rendered.contains("cand-dup"),
        "a binding refusal must not leak the payload identity: {rendered}"
    );
}

// W10-R2: the allocator seeds at 1 and skips 0 exactly once at wrap —
// the transport never emits a request id the server would refuse.
#[test]
fn request_id_allocator_never_emits_zero_across_wrap() {
    let client = QuantaIndex::from_transports(
        Arc::new(StubQueryTransport::new(SearchPlaneQueryIpcResponse::Error(
            SearchPlaneIpcError {
                code: SearchPlaneErrorCodeV2::Internal,
                message: "unused".to_string(),
                repair: None,
            },
        ))),
        unused_control(),
        unused_ingest(),
    );
    assert_eq!(
        client.test_next_request_id(),
        1,
        "a fresh allocator starts at 1, never 0"
    );
    client.test_seed_next_request_id(u64::MAX);
    assert_eq!(
        client.test_next_request_id(),
        u64::MAX,
        "the pre-wrap id is still emitted"
    );
    assert_eq!(
        client.test_next_request_id(),
        1,
        "the wrapped 0 is skipped, the sequence resumes at 1"
    );
    assert_eq!(client.test_next_request_id(), 2);
}

/// W10-R2: a transport that answers with request id 0 — the one id no
/// legitimate server emits — for testing the client's echo gate.
struct ZeroIdQueryTransport {
    response: Mutex<Option<SearchPlaneQueryIpcResponse>>,
}

impl QueryTransport for ZeroIdQueryTransport {
    fn send(
        &self,
        _request: SearchPlaneQueryIpcRequestEnvelope,
    ) -> Result<SearchPlaneQueryIpcResponseEnvelope, crate::SdkError> {
        let payload = self
            .response
            .lock()
            .map_err(|err| crate::SdkError::Protocol(format!("zero-id response poisoned: {err}")))?
            .take()
            .ok_or_else(|| crate::SdkError::Protocol("missing zero-id response".to_string()))?;
        Ok(SearchPlaneQueryIpcResponseEnvelope {
            request_id: 0,
            payload,
        })
    }
}

// W10-R2: a 0 response fails the echo check typed — the client never
// accepts the one id the server never sends.
#[test]
fn zero_response_id_fails_the_echo_check_typed() {
    let transport = Arc::new(ZeroIdQueryTransport {
        response: Mutex::new(Some(SearchPlaneQueryIpcResponse::Error(
            SearchPlaneIpcError {
                code: SearchPlaneErrorCodeV2::Internal,
                message: "the echo gate fires before the payload matters".to_string(),
                repair: None,
            },
        ))),
    });
    let client = QuantaIndex::from_transports(transport, unused_control(), unused_ingest());
    let request = sample_cluster_membership_request();
    let payload = quanta_index_contract::SearchPlaneQueryIpcRequest::ClusterMembershipRead(
        quanta_index_contract::ClusterMembershipBatchReadRequestV1::single_v1(request),
    );
    let err = match client.dispatch_query(payload) {
        Ok(response) => panic!("a 0 response must be refused, got {response:?}"),
        Err(err) => err,
    };
    let crate::SdkError::Protocol(message) = &err else {
        panic!("a 0 response must fail as Protocol, got {err:?}");
    };
    assert!(
        message.contains("request_id 0"),
        "the refusal must name the zero id, got {message}"
    );
}
