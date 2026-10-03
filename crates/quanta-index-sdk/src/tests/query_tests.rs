use super::*;

#[test]
fn cluster_membership_read_routes_exact_request_and_validates_available_authority_v1() {
    let request = sample_cluster_membership_request();
    let snapshot = quanta_index_contract::ClusterMembershipSnapshotV1 {
        cluster_record_id: request.cluster_record_id.clone(),
        generation: request.generation.clone(),
        authority_digest: request.expected_authority_digest.clone(),
        members: vec![
            SymbolId::new("symbol:auth::authenticate"),
            SymbolId::new("symbol:auth::authorize"),
        ],
        completeness: quanta_index_contract::ClusterMembershipCompletenessV1::Complete,
    };
    let expected = quanta_index_contract::ClusterMembershipReadOutcomeV1::Available(snapshot);
    let query = Arc::new(StubQueryTransport::new(
        SearchPlaneQueryIpcResponse::ClusterMembershipRead(
            quanta_index_contract::ClusterMembershipBatchReadResponseV1 {
                outcomes: vec![expected.clone()],
            },
        ),
    ));
    let client = QuantaIndex::from_transports(query.clone(), unused_control(), unused_ingest());

    let observed = ok_or_fail!(client.search().cluster_membership_read_v1(request.clone()));
    assert_eq!(observed, expected);
    let captured = ok_or_fail!(only_query_request(query.as_ref()));
    assert_eq!(
        captured.payload,
        quanta_index_contract::SearchPlaneQueryIpcRequest::ClusterMembershipRead(
            quanta_index_contract::ClusterMembershipBatchReadRequestV1::single_v1(request),
        )
    );
}

#[test]
fn cluster_membership_read_preserves_matching_typed_rejection_v1() {
    let request = sample_cluster_membership_request();
    let expected = quanta_index_contract::ClusterMembershipReadOutcomeV1::Rejected(
        quanta_index_contract::ClusterMembershipReadRejectionV1 {
            cluster_record_id: request.cluster_record_id.clone(),
            generation: request.generation.clone(),
            expected_authority_digest: request.expected_authority_digest.clone(),
            failure:
                quanta_index_contract::ClusterMembershipReadFailureV1::CurrentGenerationMissing,
        },
    );

    let query = Arc::new(StubQueryTransport::new(
        SearchPlaneQueryIpcResponse::ClusterMembershipRead(
            quanta_index_contract::ClusterMembershipBatchReadResponseV1 {
                outcomes: vec![expected.clone()],
            },
        ),
    ));
    let client = QuantaIndex::from_transports(query, unused_control(), unused_ingest());
    let observed = ok_or_fail!(client.search().cluster_membership_read_v1(request));
    assert_eq!(observed, expected);
}

#[test]
fn cluster_membership_read_rejects_stale_response_authority_v1() {
    let request = sample_cluster_membership_request();
    let stale = quanta_index_contract::ClusterMembershipReadOutcomeV1::Available(
        quanta_index_contract::ClusterMembershipSnapshotV1 {
            cluster_record_id: request.cluster_record_id.clone(),
            generation: request.generation.clone(),
            authority_digest: "stale-authority-digest".to_string(),
            members: vec![SymbolId::new("symbol:auth::authenticate")],
            completeness: quanta_index_contract::ClusterMembershipCompletenessV1::Complete,
        },
    );
    let query = Arc::new(StubQueryTransport::new(
        SearchPlaneQueryIpcResponse::ClusterMembershipRead(
            quanta_index_contract::ClusterMembershipBatchReadResponseV1 {
                outcomes: vec![stale],
            },
        ),
    ));
    let client = QuantaIndex::from_transports(query.clone(), unused_control(), unused_ingest());

    let error = match client.search().cluster_membership_read_v1(request) {
        Ok(outcome) => panic!("stale membership authority unexpectedly admitted: {outcome:?}"),
        Err(error) => error,
    };
    assert!(matches!(error, crate::SdkError::Protocol(_)));
    assert_eq!(
        ok_or_fail!(query.requests.lock()).len(),
        1,
        "authority mismatch must be detected after one exact transport read"
    );
}

#[test]
fn cluster_membership_read_rejects_mismatched_rejection_authority_v1() {
    let request = sample_cluster_membership_request();
    let mismatched_outcomes = [
        quanta_index_contract::ClusterMembershipReadOutcomeV1::Rejected(
            quanta_index_contract::ClusterMembershipReadRejectionV1 {
                cluster_record_id: "cluster-card:other".to_string(),
                generation: request.generation.clone(),
                expected_authority_digest: request.expected_authority_digest.clone(),
                failure:
                    quanta_index_contract::ClusterMembershipReadFailureV1::ClusterIdentityMismatch,
            },
        ),
        quanta_index_contract::ClusterMembershipReadOutcomeV1::Rejected(
            quanta_index_contract::ClusterMembershipReadRejectionV1 {
                cluster_record_id: request.cluster_record_id.clone(),
                generation: request.generation.clone(),
                expected_authority_digest: "stale-authority-digest".to_string(),
                failure:
                    quanta_index_contract::ClusterMembershipReadFailureV1::AuthorityDigestMismatch,
            },
        ),
    ];

    for mismatched in mismatched_outcomes {
        let query = Arc::new(StubQueryTransport::new(
            SearchPlaneQueryIpcResponse::ClusterMembershipRead(
                quanta_index_contract::ClusterMembershipBatchReadResponseV1 {
                    outcomes: vec![mismatched],
                },
            ),
        ));
        let client = QuantaIndex::from_transports(query, unused_control(), unused_ingest());
        let error = match client.search().cluster_membership_read_v1(request.clone()) {
            Ok(outcome) => {
                panic!("mismatched membership outcome authority unexpectedly admitted: {outcome:?}")
            }
            Err(error) => error,
        };
        assert!(matches!(error, crate::SdkError::Protocol(_)));
    }
}

#[test]
fn cluster_membership_read_rejects_invalid_request_before_transport_v1() {
    let mut request = sample_cluster_membership_request();
    request.limit = 0;
    let query = unused_query();
    let client = QuantaIndex::from_transports(query.clone(), unused_control(), unused_ingest());

    let error = match client.search().cluster_membership_read_v1(request) {
        Ok(outcome) => panic!("invalid membership request unexpectedly admitted: {outcome:?}"),
        Err(error) => error,
    };
    assert!(matches!(error, crate::SdkError::Usage(_)));
    assert!(
        ok_or_fail!(query.requests.lock()).is_empty(),
        "invalid request must not reach query transport"
    );
}

#[test]
fn cluster_membership_read_rejects_unrelated_query_response_v1() {
    let request = sample_cluster_membership_request();
    let query = unused_query();
    let client = QuantaIndex::from_transports(query, unused_control(), unused_ingest());

    let error = match client.search().cluster_membership_read_v1(request) {
        Ok(outcome) => panic!("unrelated query response unexpectedly admitted: {outcome:?}"),
        Err(error) => error,
    };
    let crate::SdkError::Binding { axis, .. } = error else {
        panic!("expected a binding mismatch for unrelated response");
    };
    assert_eq!(axis, crate::ResponseBindingAxis::Variant);
}

#[test]
fn cluster_membership_batch_read_routes_fifteen_items_once_and_preserves_order_v1() {
    let request = sample_cluster_membership_batch(15);
    let expected = sample_cluster_membership_batch_response(&request);
    let query = Arc::new(StubQueryTransport::new(
        SearchPlaneQueryIpcResponse::ClusterMembershipRead(expected.clone()),
    ));
    let client = QuantaIndex::from_transports(query.clone(), unused_control(), unused_ingest());

    let observed = ok_or_fail!(
        client
            .search()
            .cluster_membership_batch_read_v1(request.clone())
    );
    assert_eq!(observed, expected);
    let dispatched = {
        let requests = ok_or_fail!(query.requests.lock());
        assert_eq!(
            requests.len(),
            1,
            "one logical batch must use one transport call"
        );
        requests
            .first()
            .expect("transport call count was just asserted")
            .payload
            .clone()
    };
    assert_eq!(
        dispatched,
        quanta_index_contract::SearchPlaneQueryIpcRequest::ClusterMembershipRead(request)
    );
}

#[test]
fn cluster_membership_batch_read_rejects_partial_reordered_and_stale_response_v1() {
    let request = sample_cluster_membership_batch(3);
    let valid = sample_cluster_membership_batch_response(&request);

    let mut partial = valid.clone();
    let _removed = partial.outcomes.pop();

    let mut reordered = valid.clone();
    reordered.outcomes.swap(0, 1);

    let mut stale = valid;
    let quanta_index_contract::ClusterMembershipReadOutcomeV1::Available(snapshot) = stale
        .outcomes
        .get_mut(1)
        .expect("fixture batch must carry at least two outcomes")
    else {
        panic!("fixture must contain an available outcome")
    };
    snapshot.authority_digest.push_str(":stale");

    for malformed in [partial, reordered, stale] {
        let query = Arc::new(StubQueryTransport::new(
            SearchPlaneQueryIpcResponse::ClusterMembershipRead(malformed),
        ));
        let client = QuantaIndex::from_transports(query.clone(), unused_control(), unused_ingest());
        let error = client
            .search()
            .cluster_membership_batch_read_v1(request.clone())
            .expect_err("malformed batch response must fail the whole SDK call");
        assert!(matches!(error, crate::SdkError::Protocol(_)));
        assert_eq!(ok_or_fail!(query.requests.lock()).len(), 1);
    }
}

#[test]
fn semantic_query_builder_resolves_active_selector_before_query() {
    let query = Arc::new(StubQueryTransport::active(
        SearchPlaneQueryIpcResponse::Semantic(SemanticQueryResponse {
            generation: sample_generation_pin(),
            results: vec![sample_hit()],
            window: QueryResultWindowV2::exact_probe(1),
            explanation: sample_explanation(),
        }),
    ));
    let control = unused_control();
    let ingest = unused_ingest();
    let client = QuantaIndex::from_transports(query.clone(), control, ingest);
    let response = client
        .semantic()
        .query()
        .active(repo_id(), revision_id())
        .text("0.1 0.2 0.3")
        .top_k(5)
        .execute();
    let _response = ok_or_fail!(response);
    let captured = ok_or_fail!(query_after_resolution(query.as_ref()));
    assert!(
        matches!(
            &captured.payload,
            quanta_index_contract::SearchPlaneQueryIpcRequest::Semantic(_)
        ),
        "expected semantic request, got {:?}",
        captured.payload
    );
    let quanta_index_contract::SearchPlaneQueryIpcRequest::Semantic(req) = &captured.payload else {
        return;
    };
    assert_eq!(req.top_k, 5);
    assert_eq!(req.query_text.as_str(), "0.1 0.2 0.3");
    assert_eq!(req.generation, Some(sample_generation_pin()));
    assert!(matches!(
        req.generation_selector,
        Some(GenerationSelector::ResolvedActive { .. })
    ));
}

#[test]
fn semantic_active_keeps_catalog_selector_and_rejects_wrong_generation() {
    let wrong = quanta_index_contract::GenerationPin::new(
        repo_id(),
        revision_id(),
        ManifestGeneration::new(8),
    );
    let query = Arc::new(StubQueryTransport::sequence([
        SearchPlaneQueryIpcResponse::ActiveGenerationSnapshot(active_resolution(
            GenerationSnapshot {
                repo_id: repo_id(),
                revision_id: revision_id(),
                track: quanta_index_contract::SearchPlaneTrackKind::Semantic,
                manifest_generation: ManifestGeneration::new(7),
                manifest_digest:
                    "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                        .to_string(),
            },
        )),
        SearchPlaneQueryIpcResponse::Semantic(SemanticQueryResponse {
            generation: wrong,
            results: vec![],
            window: QueryResultWindowV2::exact_probe(0),
            explanation: sample_explanation(),
        }),
    ]));
    let client = QuantaIndex::from_transports(query.clone(), unused_control(), unused_ingest());
    let error = client
        .semantic()
        .query()
        .active(repo_id(), revision_id())
        .text("0.1 0.2 0.3")
        .top_k(5)
        .execute()
        .expect_err("same-domain wrong semantic generation must be refused");
    assert!(matches!(error, crate::SdkError::Binding { .. }));
    let requests = ok_or_fail!(query.requests.lock());
    assert!(matches!(
        requests.last().map(|request| &request.payload),
        Some(quanta_index_contract::SearchPlaneQueryIpcRequest::Semantic(request))
            if request.generation == Some(sample_generation_pin())
                && matches!(request.generation_selector, Some(GenerationSelector::ResolvedActive { .. }))
    ));
    drop(requests);
}

#[test]
fn semantic_scope_sourcegraph_query_preserves_scope_wire_fields() {
    let query = Arc::new(StubQueryTransport::new(
        SearchPlaneQueryIpcResponse::Semantic(SemanticQueryResponse {
            generation: sample_generation_pin(),
            results: vec![sample_hit()],
            window: QueryResultWindowV2::exact_probe(1),
            explanation: sample_explanation(),
        }),
    ));
    let client = QuantaIndex::from_transports(query.clone(), unused_control(), unused_ingest());
    let _response = ok_or_fail!(
        client
            .semantic()
            .query()
            .active(repo_id(), revision_id())
            .text("1.0 0.0")
            .scope_sourcegraph("repo:repo-1 file:lib.rs")
            .scope_top_k(8)
            .top_k(5)
            .execute()
    );
    let captured = ok_or_fail!(only_query_request(query.as_ref()));
    assert!(
        matches!(
            &captured.payload,
            quanta_index_contract::SearchPlaneQueryIpcRequest::Semantic(_)
        ),
        "expected semantic request, got {:?}",
        captured.payload
    );
    let quanta_index_contract::SearchPlaneQueryIpcRequest::Semantic(req) = &captured.payload else {
        return;
    };
    assert_eq!(req.top_k, 5);
    assert!(
        req.lexical_scope.is_some(),
        "expected semantic lexical scope"
    );
    let Some(scope) = &req.lexical_scope else {
        return;
    };
    assert_eq!(
        scope.syntax,
        quanta_index_contract::TextQuerySyntax::Sourcegraph
    );
    assert_eq!(scope.query_text, "repo:repo-1 file:lib.rs");
    assert_eq!(scope.top_k, 8);
}

#[test]
fn lexical_query_builder_carries_top_k_to_wire_contract() {
    let query = Arc::new(StubQueryTransport::new(SearchPlaneQueryIpcResponse::Text(
        TextQueryResponse {
            rank_unit: quanta_index_contract::TextRankUnit::Chunk,
            explanation: quanta_index_contract::SearchExplanation::empty(),
            generation: sample_generation_pin(),
            results: vec![sample_hit()],
            window: QueryResultWindowV2::exact_probe(1),
            file_owner_rows: None,
            next_cursor: None,
        },
    )));
    let client = QuantaIndex::from_transports(query.clone(), unused_control(), unused_ingest());
    let _response = ok_or_fail!(
        client
            .lexical()
            .query()
            .native("needle")
            .active(repo_id(), revision_id())
            .top_k(42)
            .execute()
    );
    let captured = ok_or_fail!(only_query_request(query.as_ref()));
    assert!(
        matches!(
            &captured.payload,
            quanta_index_contract::SearchPlaneQueryIpcRequest::Text(_)
        ),
        "expected text request, got {:?}",
        captured.payload
    );
    let quanta_index_contract::SearchPlaneQueryIpcRequest::Text(req) = &captured.payload else {
        return;
    };
    assert_eq!(
        req.top_k, 42,
        "QI-QRY-01: TextQueryRequest.top_k must be set from builder"
    );
}

#[test]
fn code_search_builder_sends_explicit_product_syntax() {
    let query = Arc::new(StubQueryTransport::new(SearchPlaneQueryIpcResponse::Text(
        TextQueryResponse {
            rank_unit: quanta_index_contract::TextRankUnit::File,
            explanation: quanta_index_contract::SearchExplanation::empty(),
            generation: sample_generation_pin(),
            results: Vec::new(),
            window: QueryResultWindowV2::exact_probe(0),
            file_owner_rows: None,
            next_cursor: None,
        },
    )));
    let client = QuantaIndex::from_transports(query.clone(), unused_control(), unused_ingest());
    let _response = ok_or_fail!(
        client
            .lexical()
            .query()
            .text("writeContent Type")
            .active(repo_id(), revision_id())
            .top_k(10)
            .execute()
    );
    let captured = ok_or_fail!(only_query_request(query.as_ref()));
    let quanta_index_contract::SearchPlaneQueryIpcRequest::Text(req) = &captured.payload else {
        panic!("expected text request, got {:?}", captured.payload);
    };
    assert_eq!(
        req.syntax,
        quanta_index_contract::TextQuerySyntax::CodeSearch
    );
    assert_eq!(req.query_text, "writeContent Type");
    assert_eq!(req.top_k, 10);
}

#[test]
fn lexical_constraint_setters_preserve_path_and_language_axes_v1() {
    let query = Arc::new(StubQueryTransport::new(SearchPlaneQueryIpcResponse::Text(
        TextQueryResponse {
            rank_unit: quanta_index_contract::TextRankUnit::Chunk,
            explanation: quanta_index_contract::SearchExplanation::empty(),
            generation: sample_generation_pin(),
            results: Vec::new(),
            window: QueryResultWindowV2::exact_probe(0),
            file_owner_rows: None,
            next_cursor: None,
        },
    )));
    let client = QuantaIndex::from_transports(query.clone(), unused_control(), unused_ingest());
    let path = ExactRepoRelativePathV1::new("src/lib.rs").expect("valid exact path");
    let rust = LanguageCode::new("rust").expect("valid language");
    let _response = ok_or_fail!(
        client
            .lexical()
            .query()
            .native("needle")
            .exact_repo_relative_path(path.clone())
            .language_any_of([rust.clone()])
            .active(repo_id(), revision_id())
            .top_k(3)
            .execute()
    );
    let captured = ok_or_fail!(only_query_request(query.as_ref()));
    assert!(
        matches!(
            &captured.payload,
            quanta_index_contract::SearchPlaneQueryIpcRequest::Text(_)
        ),
        "expected text query request, got {:?}",
        captured.payload
    );
    let quanta_index_contract::SearchPlaneQueryIpcRequest::Text(request) = &captured.payload else {
        return;
    };
    assert_eq!(
        request.constraints.repo_relative_path_exact.as_ref(),
        Some(&path),
        "language setter must not erase the exact-path axis"
    );
    assert_eq!(
        request.constraints.language_any_of,
        std::collections::BTreeSet::from([rust])
    );
}

#[test]
fn semantic_hybrid_seed_and_symbol_setters_preserve_both_constraint_axes_v1() {
    let path = ExactRepoRelativePathV1::new("src/lib.rs").expect("valid exact path");
    let rust = LanguageCode::new("rust").expect("valid language");
    let expected_languages = std::collections::BTreeSet::from([rust.clone()]);

    let semantic_transport = Arc::new(StubQueryTransport::active(
        SearchPlaneQueryIpcResponse::Semantic(SemanticQueryResponse {
            generation: sample_generation_pin(),
            results: Vec::new(),
            window: QueryResultWindowV2::exact_probe(0),
            explanation: sample_explanation(),
        }),
    ));
    let semantic_client = QuantaIndex::from_transports(
        semantic_transport.clone(),
        unused_control(),
        unused_ingest(),
    );
    let _semantic_response = ok_or_fail!(
        semantic_client
            .semantic()
            .query()
            .text("needle")
            .language_any_of([rust.clone()])
            .exact_repo_relative_path(path.clone())
            .scope_native("needle")
            .scope_top_k(3)
            .active(repo_id(), revision_id())
            .top_k(3)
            .execute()
    );
    let semantic_request = ok_or_fail!(query_after_resolution(semantic_transport.as_ref()));
    let quanta_index_contract::SearchPlaneQueryIpcRequest::Semantic(semantic_request) =
        &semantic_request.payload
    else {
        panic!("semantic builder dispatched a non-semantic request");
    };
    assert_eq!(
        semantic_request
            .constraints
            .repo_relative_path_exact
            .as_ref(),
        Some(&path)
    );
    assert_eq!(
        semantic_request.constraints.language_any_of,
        expected_languages
    );
    assert_eq!(
        semantic_request
            .lexical_scope
            .as_ref()
            .map(|scope| &scope.constraints),
        Some(&semantic_request.constraints),
        "semantic scope and dense leg must share the exact same constraint authority"
    );

    let hybrid_transport = Arc::new(StubQueryTransport::active(
        SearchPlaneQueryIpcResponse::HybridSeed(HybridSeedQueryResponse {
            generation: sample_generation_pin(),
            manifest_digest: "manifest-digest".to_string(),
            seed_candidates: Vec::new(),
            window: QueryResultWindowV2::exact_probe(0),
            explanation: sample_explanation(),
        }),
    ));
    let hybrid_client =
        QuantaIndex::from_transports(hybrid_transport.clone(), unused_control(), unused_ingest());
    let _hybrid_response = ok_or_fail!(
        hybrid_client
            .search()
            .hybrid_seed()
            .native("needle")
            .semantic_text("needle")
            .exact_repo_relative_path(path.clone())
            .language_any_of([rust.clone()])
            .active(repo_id(), revision_id())
            .top_k(3)
            .execute()
    );
    let hybrid_request = ok_or_fail!(query_after_resolution(hybrid_transport.as_ref()));
    let quanta_index_contract::SearchPlaneQueryIpcRequest::HybridSeed(hybrid_request) =
        &hybrid_request.payload
    else {
        panic!("hybrid seed builder dispatched a different request variant");
    };
    assert_eq!(
        hybrid_request
            .text_query
            .constraints
            .repo_relative_path_exact
            .as_ref(),
        Some(&path)
    );
    assert_eq!(
        hybrid_request.text_query.constraints.language_any_of,
        expected_languages
    );

    let symbol_transport = Arc::new(StubQueryTransport::active(
        SearchPlaneQueryIpcResponse::Symbol(quanta_index_contract::SymbolQueryResponse {
            generation: sample_generation_pin(),
            results: Vec::new(),
            window: QueryResultWindowV2::exact_probe(0),
            next_cursor: None,
        }),
    ));
    let symbol_client =
        QuantaIndex::from_transports(symbol_transport.clone(), unused_control(), unused_ingest());
    let _symbol_response = ok_or_fail!(
        symbol_client
            .symbol()
            .query()
            .native("")
            .language_any_of([rust])
            .exact_repo_relative_path(path.clone())
            .active(repo_id(), revision_id())
            .top_k(3)
            .execute()
    );
    let symbol_request = ok_or_fail!(query_after_resolution(symbol_transport.as_ref()));
    let quanta_index_contract::SearchPlaneQueryIpcRequest::Symbol(symbol_request) =
        &symbol_request.payload
    else {
        panic!("symbol builder dispatched a non-symbol request");
    };
    assert_eq!(
        symbol_request.constraints.repo_relative_path_exact.as_ref(),
        Some(&path)
    );
    assert_eq!(
        symbol_request.constraints.language_any_of,
        expected_languages
    );
}

#[test]
fn lexical_query_request_resolves_active_before_forwarding() {
    let query = Arc::new(StubQueryTransport::active(
        SearchPlaneQueryIpcResponse::Text(TextQueryResponse {
            rank_unit: quanta_index_contract::TextRankUnit::Chunk,
            explanation: quanta_index_contract::SearchExplanation::empty(),
            generation: sample_generation_pin(),
            results: vec![sample_hit()],
            window: QueryResultWindowV2::exact_probe(1),
            file_owner_rows: None,
            next_cursor: None,
        }),
    ));
    let client = QuantaIndex::from_transports(query.clone(), unused_control(), unused_ingest());
    let request = quanta_index_contract::TextQueryRequest {
        syntax: quanta_index_contract::TextQuerySyntax::Sourcegraph,
        query_text: "repo:repo-1 lang:rust sample".to_string(),
        constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
        generation: None,
        generation_selector: Some(GenerationSelector::Active {
            repo_id: repo_id(),
            revision_id: revision_id(),
        }),
        top_k: 13,
        cursor: None,
    };
    let _response = ok_or_fail!(client.lexical().query_request(request.clone()));
    let captured = ok_or_fail!(query_after_resolution(query.as_ref()));
    let quanta_index_contract::SearchPlaneQueryIpcRequest::Text(pinned) = captured.payload else {
        panic!("expected pinned text query");
    };
    assert_eq!(pinned.generation, Some(sample_generation_pin()));
    assert_resolved_active_selector(pinned.generation_selector.as_ref());
    assert_eq!(pinned.query_text, request.query_text);
}

#[test]
fn symbol_query_request_forwards_contract_dto_unchanged() {
    let query = Arc::new(StubQueryTransport::new(
        SearchPlaneQueryIpcResponse::Symbol(quanta_index_contract::SymbolQueryResponse {
            generation: sample_generation_pin(),
            results: vec![sample_symbol_hit()],
            window: QueryResultWindowV2::exact_probe(1),
            next_cursor: None,
        }),
    ));
    let client = QuantaIndex::from_transports(query.clone(), unused_control(), unused_ingest());
    let request = quanta_index_contract::SymbolQueryRequest {
        syntax: quanta_index_contract::TextQuerySyntax::Native,
        query_text: "symbol:sample".to_string(),
        constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
        generation: Some(sample_generation_pin()),
        generation_selector: None,
        top_k: 9,
        cursor: None,
    };
    let response = ok_or_fail!(client.symbol().query_request(request.clone()));
    assert_eq!(response.results.len(), 1);
    assert!(!response.results.is_empty(), "expected one symbol result");
    let Some(first) = response.results.first() else {
        return;
    };
    assert_eq!(first.candidate_id, "sym-1");
    assert_eq!(first.symbol_kind.as_str(), "function");
    assert_eq!(first.symbol_kind_family, Some(SymbolKindFamily::Callable));
    let captured = ok_or_fail!(only_query_request(query.as_ref()));
    assert_eq!(
        captured.payload,
        quanta_index_contract::SearchPlaneQueryIpcRequest::Symbol(request)
    );
}

#[test]
fn symbol_rev_at_time_text_does_not_relax_response_pin() {
    let wrong = quanta_index_contract::GenerationPin::new(
        repo_id(),
        revision_id(),
        ManifestGeneration::new(8),
    );
    let query = Arc::new(StubQueryTransport::new(
        SearchPlaneQueryIpcResponse::Symbol(quanta_index_contract::SymbolQueryResponse {
            generation: wrong,
            results: vec![],
            window: QueryResultWindowV2::exact_probe(0),
            next_cursor: None,
        }),
    ));
    let client = QuantaIndex::from_transports(query, unused_control(), unused_ingest());
    let request = quanta_index_contract::SymbolQueryRequest {
        syntax: quanta_index_contract::TextQuerySyntax::Native,
        query_text: "symbol:sample rev:at.time(2024-06-01T12:34:56Z)".to_string(),
        constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
        generation: Some(sample_generation_pin()),
        generation_selector: None,
        top_k: 9,
        cursor: None,
    };
    let error = client
        .symbol()
        .query_request(request)
        .expect_err("symbol route cannot rebind the generation of a lexical plan");
    assert!(matches!(error, crate::SdkError::Binding { .. }));
}

#[test]
fn hybrid_seed_search_builder_dispatches_hybrid_seed_request_with_semantic_text() {
    let query = Arc::new(StubQueryTransport::new(
        SearchPlaneQueryIpcResponse::HybridSeed(HybridSeedQueryResponse {
            generation: sample_generation_pin(),
            manifest_digest: "manifest-digest".to_string(),
            seed_candidates: vec![sample_hybrid_seed_candidate()],
            window: QueryResultWindowV2::exact_probe(1),
            explanation: sample_explanation(),
        }),
    ));
    let client = QuantaIndex::from_transports(query.clone(), unused_control(), unused_ingest());
    let _response = ok_or_fail!(
        client
            .search()
            .hybrid_seed()
            .native("scope text")
            .semantic_text("0.25 0.75")
            .active(repo_id(), revision_id())
            .dense_corpus(quanta_index_contract::SemanticCorpusKindV1::SymbolCard, 40,)
            .dense_corpus(quanta_index_contract::SemanticCorpusKindV1::ModuleCard, 20,)
            .top_k(7)
            .execute()
    );
    let captured = ok_or_fail!(only_query_request(query.as_ref()));
    assert!(
        matches!(
            &captured.payload,
            quanta_index_contract::SearchPlaneQueryIpcRequest::HybridSeed(_)
        ),
        "expected hybrid seed request, got {:?}",
        captured.payload
    );
    let quanta_index_contract::SearchPlaneQueryIpcRequest::HybridSeed(req) = &captured.payload
    else {
        return;
    };
    assert_eq!(req.top_k, 7);
    assert_eq!(req.text_query.top_k, 7);
    assert_eq!(req.semantic_query_text.as_str(), "0.25 0.75");
    assert_eq!(
        req.dense_corpora,
        vec![
            quanta_index_contract::SemanticSeedCorpusBudgetV1 {
                corpus_kind: quanta_index_contract::SemanticCorpusKindV1::SymbolCard,
                top_k: 40,
            },
            quanta_index_contract::SemanticSeedCorpusBudgetV1 {
                corpus_kind: quanta_index_contract::SemanticCorpusKindV1::ModuleCard,
                top_k: 20,
            },
        ]
    );
}

#[test]
fn semantic_query_request_resolves_active_before_forwarding() {
    let query = Arc::new(StubQueryTransport::active(
        SearchPlaneQueryIpcResponse::Semantic(SemanticQueryResponse {
            generation: sample_generation_pin(),
            results: vec![sample_hit()],
            window: QueryResultWindowV2::exact_probe(1),
            explanation: sample_explanation(),
        }),
    ));
    let client = QuantaIndex::from_transports(query.clone(), unused_control(), unused_ingest());
    let request = quanta_index_contract::SemanticQueryRequest {
        query_text: "legacy semantic text".to_string(),
        constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
        generation: None,
        generation_selector: Some(GenerationSelector::Active {
            repo_id: repo_id(),
            revision_id: revision_id(),
        }),
        lexical_scope: Some(quanta_index_contract::TextQueryRequest {
            syntax: quanta_index_contract::TextQuerySyntax::Sourcegraph,
            query_text: "repo:repo-1 file:src/lib.rs".to_string(),
            constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
            generation: Some(sample_generation_pin()),
            generation_selector: None,
            top_k: 4,
            cursor: None,
        }),
        top_k: 6,
    };
    let _response = ok_or_fail!(client.semantic().query_request(request.clone()));
    let captured = ok_or_fail!(query_after_resolution(query.as_ref()));
    let quanta_index_contract::SearchPlaneQueryIpcRequest::Semantic(pinned) = captured.payload
    else {
        panic!("expected pinned semantic query");
    };
    assert_eq!(pinned.generation, Some(sample_generation_pin()));
    assert_resolved_active_selector(pinned.generation_selector.as_ref());
    assert_eq!(pinned.query_text, request.query_text);
}

#[test]
fn hybrid_seed_request_resolves_active_before_forwarding() {
    let query = Arc::new(StubQueryTransport::active(
        SearchPlaneQueryIpcResponse::HybridSeed(HybridSeedQueryResponse {
            generation: sample_generation_pin(),
            manifest_digest: "manifest-digest".to_string(),
            seed_candidates: vec![sample_hybrid_seed_candidate()],
            window: QueryResultWindowV2::exact_probe(1),
            explanation: sample_explanation(),
        }),
    ));
    let client = QuantaIndex::from_transports(query.clone(), unused_control(), unused_ingest());
    let request = quanta_index_contract::HybridSeedQueryRequest {
        text_query: quanta_index_contract::TextQueryRequest {
            syntax: quanta_index_contract::TextQuerySyntax::Native,
            query_text: "hybrid text".to_string(),
            constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
            generation: Some(sample_generation_pin()),
            generation_selector: None,
            top_k: 11,
            cursor: None,
        },
        semantic_query_text: "legacy hybrid semantic".to_string(),
        generation: None,
        generation_selector: Some(GenerationSelector::Active {
            repo_id: repo_id(),
            revision_id: revision_id(),
        }),
        dense_corpora: Vec::new(),
        top_k: 12,
    };
    let _response = ok_or_fail!(client.search().hybrid_seed_request(request.clone()));
    let captured = ok_or_fail!(query_after_resolution(query.as_ref()));
    let quanta_index_contract::SearchPlaneQueryIpcRequest::HybridSeed(pinned) = captured.payload
    else {
        panic!("expected pinned hybrid-seed query");
    };
    assert_eq!(pinned.generation, Some(sample_generation_pin()));
    assert_resolved_active_selector(pinned.generation_selector.as_ref());
    assert_eq!(pinned.semantic_query_text, request.semantic_query_text);
}

#[test]
fn lexical_sourcegraph_query_builder_dispatches_text_query_request() {
    let query = Arc::new(StubQueryTransport::new(SearchPlaneQueryIpcResponse::Text(
        TextQueryResponse {
            rank_unit: quanta_index_contract::TextRankUnit::Chunk,
            explanation: quanta_index_contract::SearchExplanation::empty(),
            generation: sample_generation_pin(),
            results: vec![sample_hit()],
            window: QueryResultWindowV2::exact_probe(1),
            file_owner_rows: None,
            next_cursor: None,
        },
    )));
    let client = QuantaIndex::from_transports(query.clone(), unused_control(), unused_ingest());
    let response = client
        .lexical()
        .query()
        .sourcegraph("repo:repo-1 lang:rust sample")
        .pinned(sample_generation_pin())
        .top_k(9)
        .execute();
    let response = ok_or_fail!(response);
    assert_eq!(response.results.len(), 1);
    let captured = ok_or_fail!(only_query_request(query.as_ref()));
    assert!(
        matches!(
            &captured.payload,
            quanta_index_contract::SearchPlaneQueryIpcRequest::Text(_)
        ),
        "expected text request, got {:?}",
        captured.payload
    );
    let quanta_index_contract::SearchPlaneQueryIpcRequest::Text(req) = &captured.payload else {
        return;
    };
    assert_eq!(
        req.syntax,
        quanta_index_contract::TextQuerySyntax::Sourcegraph
    );
    assert_eq!(req.query_text.as_str(), "repo:repo-1 lang:rust sample");
    assert_eq!(req.generation, Some(sample_generation_pin()));
    assert_eq!(req.top_k, 9);
}
