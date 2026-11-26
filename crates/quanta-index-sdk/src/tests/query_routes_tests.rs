use super::*;

#[test]
fn history_query_routes_through_typed_query_variant() {
    let query = Arc::new(StubQueryTransport::new(
        SearchPlaneQueryIpcResponse::History(SearchPlaneHistoryQueryResponse {
            selected_active_head: None,
            generation: sample_generation_pin(),
            order: quanta_index_contract::HistoryOrderV1::Recency,
            commits: vec![],
            diffs: vec![],
            window: QueryResultWindowV2::exact_probe(0),
            read_epoch: quanta_index_contract::AuxEpochV1::new(1),
            examined: 0,
            next_cursor: None,
        }),
    ));
    let client = QuantaIndex::from_transports(query.clone(), unused_control(), unused_ingest());
    let response = ok_or_fail!(
        client
            .history()
            .query()
            .native("type:commit author:alice")
            .pinned(sample_generation_pin())
            .top_k(5)
            .order(quanta_index_contract::HistoryOrderV1::Recency)
            .execute()
    );
    assert_eq!(response.generation, sample_generation_pin());
    let captured = ok_or_fail!(only_query_request(query.as_ref()));
    assert!(
        matches!(
            &captured.payload,
            quanta_index_contract::SearchPlaneQueryIpcRequest::History(_)
        ),
        "expected History request, got {:?}",
        captured.payload
    );
    let quanta_index_contract::SearchPlaneQueryIpcRequest::History(req) = &captured.payload else {
        return;
    };
    assert_eq!(req.text_query.query_text, "type:commit author:alice");
}

#[test]
fn history_sourcegraph_query_preserves_rev_filter_and_syntax() {
    let query = Arc::new(StubQueryTransport::new(
        SearchPlaneQueryIpcResponse::History(SearchPlaneHistoryQueryResponse {
            selected_active_head: None,
            generation: sample_generation_pin(),
            order: quanta_index_contract::HistoryOrderV1::Relevance,
            commits: vec![],
            diffs: vec![],
            window: QueryResultWindowV2::exact_probe(0),
            read_epoch: quanta_index_contract::AuxEpochV1::new(1),
            examined: 0,
            next_cursor: None,
        }),
    ));
    let client = QuantaIndex::from_transports(query.clone(), unused_control(), unused_ingest());
    let _response = ok_or_fail!(
        client
            .history()
            .query()
            .sourcegraph("type:commit rev:refs/heads/main")
            .pinned(sample_generation_pin())
            .top_k(3)
            .order(quanta_index_contract::HistoryOrderV1::Relevance)
            .execute()
    );
    let captured = ok_or_fail!(only_query_request(query.as_ref()));
    assert!(
        matches!(
            &captured.payload,
            quanta_index_contract::SearchPlaneQueryIpcRequest::History(_)
        ),
        "expected History request, got {:?}",
        captured.payload
    );
    let quanta_index_contract::SearchPlaneQueryIpcRequest::History(req) = &captured.payload else {
        return;
    };
    assert_eq!(
        req.text_query.syntax,
        quanta_index_contract::TextQuerySyntax::Sourcegraph
    );
    assert_eq!(req.text_query.query_text, "type:commit rev:refs/heads/main");
}

#[test]
fn history_query_request_resolves_active_before_forwarding() {
    let query = Arc::new(StubQueryTransport::active(
        SearchPlaneQueryIpcResponse::History(SearchPlaneHistoryQueryResponse {
            selected_active_head: None,
            generation: sample_generation_pin(),
            order: quanta_index_contract::HistoryOrderV1::Relevance,
            commits: vec![],
            diffs: vec![],
            window: QueryResultWindowV2::exact_probe(0),
            read_epoch: quanta_index_contract::AuxEpochV1::new(1),
            examined: 0,
            next_cursor: None,
        }),
    ));
    let client = QuantaIndex::from_transports(query.clone(), unused_control(), unused_ingest());
    let request = HistoryQueryRequest {
        text_query: quanta_index_contract::TextQueryRequest {
            syntax: quanta_index_contract::TextQuerySyntax::Sourcegraph,
            query_text: "type:commit author:alice".to_string(),
            constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
            generation: None,
            generation_selector: Some(GenerationSelector::Active {
                repo_id: repo_id(),
                revision_id: revision_id(),
            }),
            top_k: 5,
            cursor: None,
        },
        order: quanta_index_contract::HistoryOrderV1::Relevance,
        cursor: None,
    };
    let _response = ok_or_fail!(client.history().query_request(request.clone()));
    let captured = ok_or_fail!(single_active_query(query.as_ref()));
    let quanta_index_contract::SearchPlaneQueryIpcRequest::History(pinned) = captured.payload
    else {
        panic!("expected pinned history query");
    };
    assert_eq!(pinned.text_query.generation, None);
    assert!(matches!(
        pinned.text_query.generation_selector,
        Some(GenerationSelector::Active { .. })
    ));
    assert_eq!(pinned.text_query.query_text, request.text_query.query_text);
}

#[test]
fn runtime_query_routes_through_typed_query_variant() {
    let query = Arc::new(StubQueryTransport::new(
        SearchPlaneQueryIpcResponse::RuntimeMetadata(SearchPlaneRuntimeMetadataQueryResponse {
            selected_active_head: None,
            generation: sample_generation_pin(),
            results: vec![sample_hit()],
            window: QueryResultWindowV2::exact_probe(1),
            read_epoch: quanta_index_contract::AuxEpochV1::new(1),
            universe_epoch: quanta_index_contract::AuxEpochV1::new(1),
            examined: 1,
            next_cursor: None,
        }),
    ));
    let client = QuantaIndex::from_transports(query.clone(), unused_control(), unused_ingest());
    let response = ok_or_fail!(
        client
            .runtime()
            .query()
            .sourcegraph("dirty:yes")
            .pinned(sample_generation_pin())
            .top_k(3)
            .execute()
    );
    assert_eq!(response.results.len(), 1);
    let captured = ok_or_fail!(only_query_request(query.as_ref()));
    assert!(
        matches!(
            &captured.payload,
            quanta_index_contract::SearchPlaneQueryIpcRequest::RuntimeMetadata(_)
        ),
        "expected RuntimeMetadata request, got {:?}",
        captured.payload
    );
    let quanta_index_contract::SearchPlaneQueryIpcRequest::RuntimeMetadata(req) = &captured.payload
    else {
        return;
    };
    assert_eq!(
        req.text_query.syntax,
        quanta_index_contract::TextQuerySyntax::Sourcegraph
    );
}

#[test]
fn runtime_query_request_forwards_contract_dto_unchanged() {
    let query = Arc::new(StubQueryTransport::new(
        SearchPlaneQueryIpcResponse::RuntimeMetadata(SearchPlaneRuntimeMetadataQueryResponse {
            selected_active_head: None,
            generation: sample_generation_pin(),
            results: vec![sample_hit()],
            window: QueryResultWindowV2::exact_probe(1),
            read_epoch: quanta_index_contract::AuxEpochV1::new(1),
            universe_epoch: quanta_index_contract::AuxEpochV1::new(1),
            examined: 1,
            next_cursor: None,
        }),
    ));
    let client = QuantaIndex::from_transports(query.clone(), unused_control(), unused_ingest());
    let request = RuntimeMetadataQueryRequest {
        text_query: quanta_index_contract::TextQueryRequest {
            syntax: quanta_index_contract::TextQuerySyntax::Native,
            query_text: "dirty:yes".to_string(),
            constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
            generation: Some(sample_generation_pin()),
            generation_selector: None,
            top_k: 3,
            cursor: None,
        },
        cursor: None,
    };
    let _response = ok_or_fail!(client.runtime().query_request(request.clone()));
    let captured = ok_or_fail!(only_query_request(query.as_ref()));
    assert_eq!(
        captured.payload,
        quanta_index_contract::SearchPlaneQueryIpcRequest::RuntimeMetadata(request)
    );
}

#[test]
fn structural_query_routes_through_typed_query_variant() {
    let query = Arc::new(StubQueryTransport::new(
        SearchPlaneQueryIpcResponse::Structural(SearchPlaneStructuralQueryResponse {
            generation: sample_generation_pin(),
            results: vec![],
            window: QueryResultWindowV2::exact_probe(0),
            read_epoch: quanta_index_contract::AuxEpochV1::new(1),
            examined: 0,
            next_cursor: None,
        }),
    ));
    let client = QuantaIndex::from_transports(query.clone(), unused_control(), unused_ingest());
    let response = ok_or_fail!(
        client
            .structural()
            .query()
            .native("match { :[x] }")
            .pinned(sample_generation_pin())
            .top_k(2)
            .execute()
    );
    assert_eq!(response.generation, sample_generation_pin());
    let captured = ok_or_fail!(only_query_request(query.as_ref()));
    assert!(
        matches!(
            &captured.payload,
            quanta_index_contract::SearchPlaneQueryIpcRequest::Structural(_)
        ),
        "expected Structural request, got {:?}",
        captured.payload
    );
    let quanta_index_contract::SearchPlaneQueryIpcRequest::Structural(req) = &captured.payload
    else {
        return;
    };
    assert_eq!(req.text_query.query_text, "match { :[x] }");
}

#[test]
fn structural_query_request_forwards_contract_dto_unchanged() {
    let query = Arc::new(StubQueryTransport::new(
        SearchPlaneQueryIpcResponse::Structural(SearchPlaneStructuralQueryResponse {
            generation: sample_generation_pin(),
            results: vec![],
            window: QueryResultWindowV2::exact_probe(0),
            read_epoch: quanta_index_contract::AuxEpochV1::new(1),
            examined: 0,
            next_cursor: None,
        }),
    ));
    let client = QuantaIndex::from_transports(query.clone(), unused_control(), unused_ingest());
    let request = StructuralQueryRequest {
        text_query: quanta_index_contract::TextQueryRequest {
            syntax: quanta_index_contract::TextQuerySyntax::Sourcegraph,
            query_text: r#"patterntype:structural "function_item""#.to_string(),
            constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
            generation: Some(sample_generation_pin()),
            generation_selector: None,
            top_k: 4,
            cursor: None,
        },
        cursor: None,
    };
    let _response = ok_or_fail!(client.structural().query_request(request.clone()));
    let captured = ok_or_fail!(only_query_request(query.as_ref()));
    assert_eq!(
        captured.payload,
        quanta_index_contract::SearchPlaneQueryIpcRequest::Structural(request)
    );
}

#[test]
fn structural_native_query_preserves_syntax() {
    let query = Arc::new(StubQueryTransport::new(
        SearchPlaneQueryIpcResponse::Structural(SearchPlaneStructuralQueryResponse {
            generation: sample_generation_pin(),
            results: vec![],
            window: QueryResultWindowV2::exact_probe(0),
            read_epoch: quanta_index_contract::AuxEpochV1::new(1),
            examined: 0,
            next_cursor: None,
        }),
    ));
    let client = QuantaIndex::from_transports(query.clone(), unused_control(), unused_ingest());
    let _response = ok_or_fail!(
        client
            .structural()
            .query()
            .native("repo:repo-1 lang:rust match { function_item }")
            .pinned(sample_generation_pin())
            .top_k(4)
            .execute()
    );
    let captured = ok_or_fail!(only_query_request(query.as_ref()));
    assert!(
        matches!(
            &captured.payload,
            quanta_index_contract::SearchPlaneQueryIpcRequest::Structural(_)
        ),
        "expected Structural request, got {:?}",
        captured.payload
    );
    let quanta_index_contract::SearchPlaneQueryIpcRequest::Structural(req) = &captured.payload
    else {
        return;
    };
    assert_eq!(
        req.text_query.syntax,
        quanta_index_contract::TextQuerySyntax::Native
    );
    assert_eq!(
        req.text_query.query_text,
        "repo:repo-1 lang:rust match { function_item }"
    );
    assert_eq!(req.text_query.top_k, 4);
}

#[test]
fn structural_sourcegraph_query_preserves_syntax() {
    let query = Arc::new(StubQueryTransport::new(
        SearchPlaneQueryIpcResponse::Structural(SearchPlaneStructuralQueryResponse {
            generation: sample_generation_pin(),
            results: vec![],
            window: QueryResultWindowV2::exact_probe(0),
            read_epoch: quanta_index_contract::AuxEpochV1::new(1),
            examined: 0,
            next_cursor: None,
        }),
    ));
    let client = QuantaIndex::from_transports(query.clone(), unused_control(), unused_ingest());
    let _response =
        ok_or_fail!(client
        .structural()
        .query()
        .sourcegraph(
            r#"repo:repo-1 path:src/lib.rs lang:rust patterntype:structural "function_item""#
        )
        .pinned(sample_generation_pin())
        .top_k(4)
        .execute());
    let captured = ok_or_fail!(only_query_request(query.as_ref()));
    assert!(
        matches!(
            &captured.payload,
            quanta_index_contract::SearchPlaneQueryIpcRequest::Structural(_)
        ),
        "expected Structural request, got {:?}",
        captured.payload
    );
    let quanta_index_contract::SearchPlaneQueryIpcRequest::Structural(req) = &captured.payload
    else {
        return;
    };
    assert_eq!(
        req.text_query.syntax,
        quanta_index_contract::TextQuerySyntax::Sourcegraph
    );
    assert_eq!(
        req.text_query.query_text,
        r#"repo:repo-1 path:src/lib.rs lang:rust patterntype:structural "function_item""#
    );
    assert_eq!(req.text_query.top_k, 4);
}

#[test]
fn lexical_publish_propagates_ingest_error_as_typed_remote() {
    let ingest = Arc::new(StubIngestTransport::new(
        SearchPlaneIngestIpcResponse::Error(quanta_index_contract::SearchPlaneIpcError {
            code: SearchPlaneErrorCodeV2::InvalidRequest,
            message: "channel rejected".to_string(),
            repair: None,
        }),
    ));
    let client = QuantaIndex::from_transports(unused_query(), unused_control(), ingest);
    let batch = SearchCorpusBatch::replace_generation(
        repo_id(),
        revision_id(),
        ManifestGeneration::new(1),
        "manifest:feed",
    )
    .source_event(sample_source_event());
    let err = client.search_corpus().publish(&batch).err();
    assert!(
        matches!(err, Some(crate::SdkError::Remote { .. })),
        "expected Remote error, got {err:?}"
    );
    let Some(crate::SdkError::Remote { code, message, .. }) = err else {
        return;
    };
    assert_eq!(code, SearchPlaneErrorCodeV2::InvalidRequest);
    assert!(message.contains("channel rejected"));
}
