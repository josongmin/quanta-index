use super::*;

#[test]
fn repomap_query_routes_through_query_transport() {
    let response = sample_repomap_query_response();
    let query = Arc::new(StubQueryTransport::new(
        SearchPlaneQueryIpcResponse::RepoMapQuery(response.clone()),
    ));
    let client = QuantaIndex::from_transports(query.clone(), unused_control(), unused_ingest());
    let request = sample_repomap_query_request();
    let observed = ok_or_fail!(client.repomap().query(request.clone()));
    assert_eq!(observed, response);
    let captured = ok_or_fail!(only_query_request(query.as_ref()));
    assert!(
        matches!(
            &captured.payload,
            quanta_index_contract::SearchPlaneQueryIpcRequest::RepoMapQuery(_)
        ),
        "expected RepoMapQuery request, got {:?}",
        captured.payload
    );
    let quanta_index_contract::SearchPlaneQueryIpcRequest::RepoMapQuery(wire) = &captured.payload
    else {
        return;
    };
    assert_eq!(wire.query_text, request.query_text);
    assert_eq!(wire.top_k, request.top_k);
    assert_eq!(wire.token_budget, request.token_budget);
    assert_eq!(wire.focus_subjects, request.focus_subjects);
}

#[test]
fn repomap_activate_routes_through_control_transport() {
    let ack = RepoMapMutationAck {
        prior_candidate_commitment: None,
        new_candidate_commitment: format!("sha256:{}", "ab".repeat(32)),
        activation_epoch: 1,
        terminal_sequence: 1,
        replayed: false,
        repo_id: repo_id(),
        revision_id: revision_id(),
        manifest_generation: ManifestGeneration::new(9),
    };
    let request = quanta_index_contract::RepoMapActivateGenerationRequestV2 {
        repo_id: repo_id(),
        revision_id: revision_id(),
        manifest_generation: ManifestGeneration::new(9),
        manifest_digest: "digest:repomap-9".to_string(),
        snapshot_id: "snap-9".to_string(),
        projection_version: 1,
        authority_digest: "authority-9".to_string(),
        source_bundle_digest: format!("sha256:{}", "cd".repeat(32)),
        expected_active: None,
    };
    let receipt = quanta_index_contract::RepoMapTerminalReceiptV2 {
        phase: quanta_index_contract::RepoMapMutationPhaseV2::Activate,
        mutation: ack,
        manifest_digest: request.manifest_digest.clone(),
        snapshot_id: request.snapshot_id.clone(),
        projection_version: request.projection_version,
        authority_digest: request.authority_digest.clone(),
        source_bundle_digest: request.source_bundle_digest.clone(),
    };
    let control = Arc::new(StubControlTransport::new(
        quanta_index_contract::SearchPlaneControlIpcResponse::RepoMapTerminalReceiptV2(
            receipt.clone(),
        ),
    ));
    let client = QuantaIndex::from_transports(unused_query(), control.clone(), unused_ingest());
    let observed = ok_or_fail!(client.repomap().activate(request.clone()));
    assert_eq!(observed, receipt);
    let captured = ok_or_fail!(only_control_request(control.as_ref()));
    assert!(
        matches!(
            &captured.payload,
            quanta_index_contract::SearchPlaneControlIpcRequest::RepoMapActivateV2(_)
        ),
        "expected RepoMapActivateV2 request, got {:?}",
        captured.payload
    );
    let quanta_index_contract::SearchPlaneControlIpcRequest::RepoMapActivateV2(wire) =
        &captured.payload
    else {
        return;
    };
    assert_eq!(wire, &request);
}

#[test]
fn repomap_active_head_reads_control_and_rejects_a_foreign_pair() {
    let expected = quanta_index_contract::RepoMapExpectedActiveV2::new(
        std::num::NonZeroU64::new(7).expect("positive epoch"),
        quanta_index_contract::CandidateCommitmentV1::from_bytes([0xab; 32]),
    );
    let response = quanta_index_contract::RepoMapActiveHeadResponseV2 {
        repo_id: repo_id(),
        revision_id: revision_id(),
        active: Some(expected.clone()),
    };
    let control = Arc::new(StubControlTransport::new(
        quanta_index_contract::SearchPlaneControlIpcResponse::RepoMapActiveHeadV2(response.clone()),
    ));
    let client = QuantaIndex::from_transports(unused_query(), control.clone(), unused_ingest());
    assert_eq!(
        ok_or_fail!(client.repomap().active_head(repo_id(), revision_id())),
        Some(expected)
    );
    let captured = ok_or_fail!(only_control_request(control.as_ref()));
    assert!(matches!(
        captured.payload,
        quanta_index_contract::SearchPlaneControlIpcRequest::RepoMapActiveHeadV2(ref request)
            if request.repo_id == repo_id() && request.revision_id == revision_id()
    ));

    let foreign = quanta_index_contract::RepoMapActiveHeadResponseV2 {
        revision_id: RevisionId::new("foreign").expect("canonical revision"),
        ..response
    };
    let control = Arc::new(StubControlTransport::new(
        quanta_index_contract::SearchPlaneControlIpcResponse::RepoMapActiveHeadV2(foreign),
    ));
    let client = QuantaIndex::from_transports(unused_query(), control, unused_ingest());
    assert!(matches!(
        client.repomap().active_head(repo_id(), revision_id()),
        Err(crate::SdkError::Binding {
            axis: crate::ResponseBindingAxis::TargetIdentity,
            ..
        })
    ));
}

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
