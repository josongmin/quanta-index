use super::*;

#[test]
fn active_resolution_rejects_wrong_same_domain_query_generation() {
    let resolved = GenerationSnapshot {
        repo_id: repo_id(),
        revision_id: revision_id(),
        track: quanta_index_contract::SearchPlaneTrackKind::Lexical,
        manifest_generation: ManifestGeneration::new(7),
        manifest_digest: "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            .to_string(),
    };
    let wrong = quanta_index_contract::GenerationPin::new(
        repo_id(),
        revision_id(),
        ManifestGeneration::new(8),
    );
    let query = Arc::new(StubQueryTransport::sequence([
        SearchPlaneQueryIpcResponse::ActiveGenerationSnapshot(active_resolution(resolved)),
        SearchPlaneQueryIpcResponse::Text(TextQueryResponse {
            rank_unit: quanta_index_contract::TextRankUnit::Chunk,
            explanation: quanta_index_contract::SearchExplanation::empty(),
            generation: wrong,
            results: Vec::new(),
            window: QueryResultWindowV2::exact_probe(0),
            file_owner_rows: None,
            next_cursor: None,
        }),
    ]));
    let client = QuantaIndex::from_transports(query.clone(), unused_control(), unused_ingest());
    let request = quanta_index_contract::TextQueryRequest {
        syntax: quanta_index_contract::TextQuerySyntax::Native,
        query_text: "needle".to_string(),
        constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
        generation: None,
        generation_selector: Some(GenerationSelector::Active {
            repo_id: repo_id(),
            revision_id: revision_id(),
        }),
        top_k: 5,
        cursor: None,
    };
    let error = client
        .lexical()
        .query_request(request)
        .expect_err("wrong same-domain generation must not satisfy an active request");
    assert!(matches!(error, crate::SdkError::Binding { .. }));
    let requests = ok_or_fail!(query.requests.lock());
    assert!(matches!(
        requests.first().map(|request| &request.payload),
        Some(quanta_index_contract::SearchPlaneQueryIpcRequest::ResolveActiveGeneration(_))
    ));
    assert!(matches!(
        requests.last().map(|request| &request.payload),
        Some(quanta_index_contract::SearchPlaneQueryIpcRequest::Text(request))
            if request.generation == Some(sample_generation_pin())
                && matches!(
                    request.generation_selector.as_ref(),
                    Some(GenerationSelector::ResolvedActive { activation_token, .. })
                        if activation_token.root_incarnation() == [7; 16]
                            && activation_token.activation_sequence().get() == 1
                )
    ));
    drop(requests);
}

#[test]
fn resolved_active_without_explicit_pin_binds_the_final_text_generation() {
    let snapshot = GenerationSnapshot {
        repo_id: repo_id(),
        revision_id: revision_id(),
        track: Track::Lexical,
        manifest_generation: ManifestGeneration::new(7),
        manifest_digest: "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            .to_string(),
    };
    let resolution = active_resolution(snapshot);
    let token = resolution.head.activation_token;
    let request = quanta_index_contract::TextQueryRequest {
        syntax: quanta_index_contract::TextQuerySyntax::Native,
        query_text: "needle".to_string(),
        constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
        generation: None,
        generation_selector: Some(GenerationSelector::ResolvedActive {
            repo_id: repo_id(),
            revision_id: revision_id(),
            activation_token: token,
        }),
        top_k: 5,
        cursor: None,
    };
    let wrong = quanta_index_contract::GenerationPin::new(
        repo_id(),
        revision_id(),
        ManifestGeneration::new(8),
    );
    for final_pin in [sample_generation_pin(), wrong] {
        let query = Arc::new(StubQueryTransport::sequence([
            SearchPlaneQueryIpcResponse::ActiveGenerationSnapshot(resolution.clone()),
            SearchPlaneQueryIpcResponse::Text(TextQueryResponse {
                rank_unit: quanta_index_contract::TextRankUnit::Chunk,
                explanation: quanta_index_contract::SearchExplanation::empty(),
                generation: final_pin.clone(),
                results: Vec::new(),
                window: QueryResultWindowV2::exact_probe(0),
                file_owner_rows: None,
                next_cursor: None,
            }),
        ]));
        let client = QuantaIndex::from_transports(query.clone(), unused_control(), unused_ingest());
        let result = client.lexical().query_request(request.clone());
        if final_pin == sample_generation_pin() {
            assert_eq!(ok_or_fail!(result).generation, final_pin);
        } else {
            assert!(matches!(
                result,
                Err(crate::SdkError::Binding {
                    axis: crate::ResponseBindingAxis::ReadIdentity,
                    ..
                })
            ));
        }
        let requests = ok_or_fail!(query.requests.lock());
        assert_eq!(requests.len(), 2);
        assert!(matches!(
            requests.first().map(|request| &request.payload),
            Some(quanta_index_contract::SearchPlaneQueryIpcRequest::ResolveActiveGeneration(_))
        ));
        assert!(matches!(
            requests.last().map(|request| &request.payload),
            Some(quanta_index_contract::SearchPlaneQueryIpcRequest::Text(sent))
                if sent.generation == Some(sample_generation_pin())
                    && sent.generation_selector == request.generation_selector
        ));
        drop(requests);
    }
}

#[test]
fn resolved_active_without_explicit_pin_refuses_a_changed_activation_token() {
    let snapshot = GenerationSnapshot {
        repo_id: repo_id(),
        revision_id: revision_id(),
        track: Track::Lexical,
        manifest_generation: ManifestGeneration::new(7),
        manifest_digest: "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            .to_string(),
    };
    let resolution = active_resolution(snapshot);
    let different_token = SearchCorpusActivationTokenV1::new(
        [7; 16],
        NonZeroU64::new(2).expect("fixture activation sequence is positive"),
    )
    .expect("fixture incarnation is nonzero");
    let query = Arc::new(StubQueryTransport::sequence([
        SearchPlaneQueryIpcResponse::ActiveGenerationSnapshot(resolution),
    ]));
    let client = QuantaIndex::from_transports(query.clone(), unused_control(), unused_ingest());
    let request = quanta_index_contract::TextQueryRequest {
        syntax: quanta_index_contract::TextQuerySyntax::Native,
        query_text: "needle".to_string(),
        constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
        generation: None,
        generation_selector: Some(GenerationSelector::ResolvedActive {
            repo_id: repo_id(),
            revision_id: revision_id(),
            activation_token: different_token,
        }),
        top_k: 5,
        cursor: None,
    };
    assert!(matches!(
        client.lexical().query_request(request),
        Err(crate::SdkError::Remote {
            code: SearchPlaneErrorCodeV2::NotReady,
            ..
        })
    ));
    let requests = ok_or_fail!(query.requests.lock());
    assert_eq!(
        requests.len(),
        1,
        "a stale token must not send a text query"
    );
    drop(requests);
}

#[test]
fn lexical_time_resolution_binds_the_final_ancestor_pin() {
    let ancestor = quanta_index_contract::GenerationPin::new(
        repo_id(),
        RevisionId::new("ancestor").expect("fixture revision"),
        ManifestGeneration::new(3),
    );
    let request = quanta_index_contract::TextQueryRequest {
        syntax: quanta_index_contract::TextQuerySyntax::Sourcegraph,
        query_text: "needle rev:at.time(2024-06-01T12:34:56Z)".to_string(),
        constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
        generation: Some(sample_generation_pin()),
        generation_selector: None,
        top_k: 5,
        cursor: None,
    };
    for final_pin in [ancestor.clone(), sample_generation_pin()] {
        let query = Arc::new(StubQueryTransport::sequence([
            SearchPlaneQueryIpcResponse::ResolvedLexicalGeneration(ancestor.clone()),
            SearchPlaneQueryIpcResponse::Text(TextQueryResponse {
                rank_unit: quanta_index_contract::TextRankUnit::Chunk,
                explanation: quanta_index_contract::SearchExplanation::empty(),
                generation: final_pin.clone(),
                results: vec![],
                window: QueryResultWindowV2::exact_probe(0),
                file_owner_rows: None,
                next_cursor: None,
            }),
        ]));
        let client = QuantaIndex::from_transports(query.clone(), unused_control(), unused_ingest());
        let result = client.lexical().query_request(request.clone());
        if final_pin == ancestor {
            assert_eq!(ok_or_fail!(result).generation, ancestor);
        } else {
            assert!(matches!(result, Err(crate::SdkError::Binding { .. })));
        }
        let requests = ok_or_fail!(query.requests.lock());
        assert!(matches!(
            requests.first().map(|request| &request.payload),
            Some(quanta_index_contract::SearchPlaneQueryIpcRequest::ResolveLexicalGeneration(resolved))
                if resolved == &request
        ));
        assert!(matches!(
            requests.last().map(|request| &request.payload),
            Some(quanta_index_contract::SearchPlaneQueryIpcRequest::Text(sent))
                if sent == &request
        ));
        drop(requests);
    }
}

#[test]
fn quoted_timeref_literals_do_not_resolve_or_relax_text_response_pins() {
    let literal = "\"rev:at.time(2024-06-01T12:34:56Z)\"";
    assert!(!crate::binding::is_rev_at_time_query(
        quanta_index_contract::TextQuerySyntax::CodeSearch,
        literal
    ));
    assert!(crate::binding::is_rev_at_time_query(
        quanta_index_contract::TextQuerySyntax::Sourcegraph,
        "rev:at.time(2024-06-01T12:34:56Z) needle"
    ));

    let wrong = quanta_index_contract::GenerationPin::new(
        repo_id(),
        RevisionId::new("other-revision").expect("fixture revision"),
        ManifestGeneration::new(8),
    );
    let query = Arc::new(StubQueryTransport::new(SearchPlaneQueryIpcResponse::Text(
        TextQueryResponse {
            rank_unit: quanta_index_contract::TextRankUnit::File,
            explanation: quanta_index_contract::SearchExplanation::empty(),
            generation: wrong,
            results: vec![],
            window: QueryResultWindowV2::exact_probe(0),
            file_owner_rows: None,
            next_cursor: None,
        },
    )));
    let client = QuantaIndex::from_transports(query.clone(), unused_control(), unused_ingest());
    let request = quanta_index_contract::TextQueryRequest {
        syntax: quanta_index_contract::TextQuerySyntax::CodeSearch,
        query_text: literal.to_string(),
        constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
        generation: Some(sample_generation_pin()),
        generation_selector: None,
        top_k: 5,
        cursor: None,
    };
    let error = client
        .lexical()
        .query_request(request.clone())
        .expect_err("a code-search literal cannot authorize another response pin");
    assert!(matches!(error, crate::SdkError::Binding { .. }));
    let requests = ok_or_fail!(query.requests.lock());
    assert_eq!(requests.len(), 1, "a literal must not trigger resolution");
    assert!(matches!(
        requests.first().map(|request| &request.payload),
        Some(quanta_index_contract::SearchPlaneQueryIpcRequest::Text(sent)) if sent == &request
    ));
    drop(requests);
}

#[test]
fn code_search_resolution_response_cannot_rebind_a_literal() {
    let wrong = quanta_index_contract::GenerationPin::new(
        repo_id(),
        RevisionId::new("other-revision").expect("fixture revision"),
        ManifestGeneration::new(8),
    );
    let query = Arc::new(StubQueryTransport::new(
        SearchPlaneQueryIpcResponse::ResolvedLexicalGeneration(wrong),
    ));
    let client = QuantaIndex::from_transports(query, unused_control(), unused_ingest());
    let request = quanta_index_contract::TextQueryRequest {
        syntax: quanta_index_contract::TextQuerySyntax::CodeSearch,
        query_text: "\"rev:at.time(2024-06-01T12:34:56Z)\"".to_string(),
        constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
        generation: Some(sample_generation_pin()),
        generation_selector: None,
        top_k: 5,
        cursor: None,
    };
    let error = client
        .dispatch_query(
            quanta_index_contract::SearchPlaneQueryIpcRequest::ResolveLexicalGeneration(request),
        )
        .expect_err("code search resolution cannot attest a different pin");
    assert!(matches!(error, crate::SdkError::Binding { .. }));
}

#[test]
fn lexical_time_resolution_rejects_foreign_repo_before_query() {
    let foreign = quanta_index_contract::GenerationPin::new(
        RepoId::new("foreign").expect("fixture repo"),
        RevisionId::new("ancestor").expect("fixture revision"),
        ManifestGeneration::new(3),
    );
    let query = Arc::new(StubQueryTransport::sequence([
        SearchPlaneQueryIpcResponse::ResolvedLexicalGeneration(foreign),
    ]));
    let client = QuantaIndex::from_transports(query.clone(), unused_control(), unused_ingest());
    let request = quanta_index_contract::TextQueryRequest {
        syntax: quanta_index_contract::TextQuerySyntax::Sourcegraph,
        query_text: "needle rev:at.time(2024-06-01T12:34:56Z)".to_string(),
        constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
        generation: Some(sample_generation_pin()),
        generation_selector: None,
        top_k: 5,
        cursor: None,
    };
    let error = client
        .lexical()
        .query_request(request)
        .expect_err("foreign lexical resolution must be refused");
    assert!(matches!(error, crate::SdkError::Binding { .. }));
    let requests = ok_or_fail!(query.requests.lock());
    assert_eq!(requests.len(), 1, "final query must not be dispatched");
    drop(requests);
}
