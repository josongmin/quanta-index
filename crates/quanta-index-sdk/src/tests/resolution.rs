use super::*;

#[test]
fn active_query_requires_a_selected_head_from_the_query_rpc() {
    let query = Arc::new(StubQueryTransport::new(SearchPlaneQueryIpcResponse::Text(
        TextQueryResponse {
            generation: sample_generation_pin(),
            selected_active_head: None,
            rank_unit: quanta_index_contract::TextRankUnit::Chunk,
            results: vec![],
            window: QueryResultWindowV2::exact_probe(0),
            explanation: quanta_index_contract::SearchExplanation::empty(),
            file_owner_rows: None,
            next_cursor: None,
        },
    )));
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
    assert!(matches!(
        client.lexical().query_request(request),
        Err(crate::SdkError::Binding {
            axis: crate::ResponseBindingAxis::ReadIdentity,
            ..
        })
    ));
    assert_eq!(ok_or_fail!(query.requests.lock()).len(), 1);
}

#[test]
fn active_query_is_one_rpc_and_rejects_wrong_same_domain_generation() {
    let wrong = GenerationPin::new(repo_id(), revision_id(), ManifestGeneration::new(8));
    let query = Arc::new(StubQueryTransport::sequence([
        SearchPlaneQueryIpcResponse::Text(TextQueryResponse {
            selected_active_head: Some(search_corpus_head(
                7,
                "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                1,
            )),
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
    assert!(matches!(
        client.lexical().query_request(request.clone()),
        Err(crate::SdkError::Binding { .. })
    ));
    let requests = ok_or_fail!(query.requests.lock());
    assert_eq!(requests.len(), 1);
    assert!(
        matches!(&requests[0].payload, quanta_index_contract::SearchPlaneQueryIpcRequest::Text(sent) if sent == &request)
    );
}

#[test]
fn resolved_active_binds_generation_and_activation_token_in_one_rpc() {
    let head = search_corpus_head(
        7,
        "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        1,
    );
    let wrong = GenerationPin::new(repo_id(), revision_id(), ManifestGeneration::new(8));
    for (final_pin, expected_ok) in [(sample_generation_pin(), true), (wrong, false)] {
        let query = Arc::new(StubQueryTransport::sequence([
            SearchPlaneQueryIpcResponse::Text(TextQueryResponse {
                selected_active_head: Some(head.clone()),
                rank_unit: quanta_index_contract::TextRankUnit::Chunk,
                explanation: quanta_index_contract::SearchExplanation::empty(),
                generation: final_pin,
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
            generation_selector: Some(GenerationSelector::ResolvedActive {
                repo_id: repo_id(),
                revision_id: revision_id(),
                activation_token: head.activation_token,
            }),
            top_k: 5,
            cursor: None,
        };
        let result = client.lexical().query_request(request.clone());
        assert_eq!(result.is_ok(), expected_ok);
        let requests = ok_or_fail!(query.requests.lock());
        assert_eq!(requests.len(), 1);
        assert!(
            matches!(&requests[0].payload, quanta_index_contract::SearchPlaneQueryIpcRequest::Text(sent) if sent == &request)
        );
    }
}

#[test]
fn resolved_active_refuses_a_changed_activation_token() {
    let head = search_corpus_head(
        7,
        "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        1,
    );
    let different_token =
        SearchCorpusActivationTokenV1::new([7; 16], NonZeroU64::new(2).expect("positive sequence"))
            .expect("valid token");
    let query = Arc::new(StubQueryTransport::sequence([
        SearchPlaneQueryIpcResponse::Text(TextQueryResponse {
            selected_active_head: Some(head),
            rank_unit: quanta_index_contract::TextRankUnit::Chunk,
            explanation: quanta_index_contract::SearchExplanation::empty(),
            generation: sample_generation_pin(),
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
        Err(crate::SdkError::Binding {
            axis: crate::ResponseBindingAxis::SelectorDomain,
            ..
        })
    ));
    assert_eq!(ok_or_fail!(query.requests.lock()).len(), 1);
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
                selected_active_head: None,
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
fn active_rev_at_time_keeps_ancestor_resolution_and_exact_final_binding() {
    let ancestor = GenerationPin::new(
        repo_id(),
        RevisionId::new("ancestor").expect("fixture revision"),
        ManifestGeneration::new(3),
    );
    let query = Arc::new(StubQueryTransport::sequence([
        SearchPlaneQueryIpcResponse::ResolvedLexicalGeneration(ancestor.clone()),
        SearchPlaneQueryIpcResponse::Text(TextQueryResponse {
            generation: ancestor.clone(),
            selected_active_head: None,
            rank_unit: quanta_index_contract::TextRankUnit::Chunk,
            results: vec![],
            window: QueryResultWindowV2::exact_probe(0),
            explanation: quanta_index_contract::SearchExplanation::empty(),
            file_owner_rows: None,
            next_cursor: None,
        }),
    ]));
    let client = QuantaIndex::from_transports(query.clone(), unused_control(), unused_ingest());
    let request = quanta_index_contract::TextQueryRequest {
        syntax: quanta_index_contract::TextQuerySyntax::Sourcegraph,
        query_text: "needle rev:at.time(2024-06-01T12:34:56Z)".to_string(),
        constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
        generation: None,
        generation_selector: Some(GenerationSelector::Active {
            repo_id: repo_id(),
            revision_id: revision_id(),
        }),
        top_k: 5,
        cursor: None,
    };
    assert_eq!(
        ok_or_fail!(client.lexical().query_request(request.clone())).generation,
        ancestor
    );
    let requests = ok_or_fail!(query.requests.lock());
    assert_eq!(requests.len(), 2);
    assert!(
        matches!(&requests[0].payload, quanta_index_contract::SearchPlaneQueryIpcRequest::ResolveLexicalGeneration(sent) if sent == &request)
    );
    assert!(
        matches!(&requests[1].payload, quanta_index_contract::SearchPlaneQueryIpcRequest::Text(sent) if sent == &request)
    );
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
            selected_active_head: None,
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
