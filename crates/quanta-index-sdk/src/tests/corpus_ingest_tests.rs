use super::*;

#[test]
fn search_corpus_publish_routes_through_ingest_transport_and_carries_typed_records() {
    let receipt = BatchPublishReceipt {
        generation: ManifestGeneration::new(1),
        manifest_digest: Some("manifest:feed".to_string()),
        batch_digest: String::new(),
        applied: true,
        durable_sequence: 7,
        semantic_content: None,
        accepted_clear_surfaces: 0,
        accepted_replace_scopes: 1,
        accepted_tombstone_scopes: 0,
        accepted_semantic_replace_scopes: 0,
        accepted_semantic_tombstone_scopes: 0,
        sealed: true,
    };
    let ingest = Arc::new(StubIngestTransport::for_corpus_receipt(receipt.clone()));
    let client = QuantaIndex::from_transports(unused_query(), unused_control(), ingest.clone());
    let chunk = sample_chunk();
    let symbol = sample_symbol();
    let batch = SearchCorpusBatch::replace_generation(
        repo_id(),
        revision_id(),
        ManifestGeneration::new(1),
        "manifest:feed",
    )
    .source_event(sample_source_event())
    .replace_scope(
        sample_source_coverage(),
        b"fn sample() {}".to_vec(),
        vec![chunk.clone()],
        vec![symbol.clone()],
    );
    let observed = ok_or_fail!(client.search_corpus().publish(&batch));
    let expected = BatchPublishReceipt {
        batch_digest: ok_or_fail!(batch.batch_digest()),
        ..receipt
    };
    assert_eq!(observed, expected);
    let captured = ok_or_fail!(only_ingest_request(ingest.as_ref()));
    assert!(
        matches!(
            &captured.payload,
            SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(_)
        ),
        "expected PublishSearchCorpusBatch, got {:?}",
        captured.payload
    );
    let SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(wire) = &captured.payload else {
        return;
    };
    assert_eq!(wire.repo_id, repo_id());
    assert_eq!(wire.manifest_digest, "manifest:feed");
    assert_eq!(wire.batch_digest, ok_or_fail!(batch.batch_digest()));
    assert_eq!(wire.replace_scopes.len(), 1);
    assert_eq!(wire.tombstone_scopes.len(), 0);
    assert!(wire.seal);
    assert_eq!(
        wire.replace_scopes.len(),
        1,
        "expected one search corpus replace scope"
    );
    let Some(first_scope) = wire.replace_scopes.first() else {
        return;
    };
    assert_eq!(first_scope.coverage, sample_source_coverage());
    assert_eq!(first_scope.chunks, vec![chunk]);
    assert_eq!(first_scope.symbols, vec![symbol]);
}

#[test]
fn search_corpus_builder_preserves_semantic_lifecycle_in_canonical_wire_order() {
    let receipt = BatchPublishReceipt {
        generation: ManifestGeneration::new(1),
        manifest_digest: Some("manifest:semantic".to_string()),
        batch_digest: String::new(),
        applied: true,
        durable_sequence: 7,
        semantic_content: None,
        accepted_clear_surfaces: 0,
        accepted_replace_scopes: 0,
        accepted_tombstone_scopes: 0,
        accepted_semantic_replace_scopes: 2,
        accepted_semantic_tombstone_scopes: 2,
        sealed: true,
    };
    let ingest = Arc::new(StubIngestTransport::for_corpus_receipt(receipt));
    let client = QuantaIndex::from_transports(unused_query(), unused_control(), ingest.clone());
    let scope_a = sample_semantic_scope("symbol-a");
    let scope_b = sample_semantic_scope("symbol-b");
    let tombstone_c = sample_semantic_scope("symbol-c");
    let tombstone_d = sample_semantic_scope("symbol-d");
    let batch = SearchCorpusBatch::replace_generation(
        repo_id(),
        revision_id(),
        ManifestGeneration::new(1),
        "manifest:semantic",
    )
    .source_event(sample_source_event())
    .replace_semantic_scope(
        scope_b.clone(),
        "scope:b",
        vec![sample_semantic_source("symbol-b")],
        Vec::new(),
    )
    .replace_semantic_scope(
        scope_a.clone(),
        "scope:a",
        vec![sample_semantic_source("symbol-a")],
        Vec::new(),
    )
    .tombstone_semantic_scope(tombstone_d.clone())
    .tombstone_semantic_scope(tombstone_c.clone());

    assert_eq!(
        batch
            .semantic_replace_scopes()
            .iter()
            .map(|mutation| mutation.scope.clone())
            .collect::<Vec<_>>(),
        vec![scope_a, scope_b]
    );
    assert_eq!(
        batch.semantic_tombstone_scopes(),
        &[tombstone_c, tombstone_d]
    );

    let _receipt = ok_or_fail!(client.search_corpus().publish(&batch));
    let captured = ok_or_fail!(only_ingest_request(ingest.as_ref()));
    let SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(wire) = captured.payload else {
        panic!("expected search corpus wire batch");
    };
    assert_eq!(
        wire.semantic_replace_scopes,
        batch.semantic_replace_scopes()
    );
    assert_eq!(
        wire.semantic_tombstone_scopes,
        batch.semantic_tombstone_scopes()
    );

    let unsealed = batch.without_seal();
    assert!(!unsealed.seal_requested());
    assert_eq!(unsealed.semantic_replace_scopes().len(), 2);
    assert_eq!(unsealed.semantic_tombstone_scopes().len(), 2);
}

#[test]
fn search_corpus_builder_preserves_typed_cluster_membership_without_text_inference_v1() {
    let receipt = BatchPublishReceipt {
        generation: ManifestGeneration::new(1),
        manifest_digest: Some("manifest:cluster".to_string()),
        batch_digest: String::new(),
        applied: true,
        durable_sequence: 7,
        semantic_content: None,
        accepted_clear_surfaces: 0,
        accepted_replace_scopes: 0,
        accepted_tombstone_scopes: 0,
        accepted_semantic_replace_scopes: 1,
        accepted_semantic_tombstone_scopes: 0,
        sealed: true,
    };
    let ingest = Arc::new(StubIngestTransport::for_corpus_receipt(receipt));
    let client = QuantaIndex::from_transports(unused_query(), unused_control(), ingest.clone());
    let source_a = sample_cluster_semantic_source("auth-service", "a");
    let source_b = sample_cluster_semantic_source("auth-service", "b");
    let membership_a = sample_cluster_membership(&source_a, "a");
    let membership_b = sample_cluster_membership(&source_b, "b");
    let batch = SearchCorpusBatch::replace_generation(
        repo_id(),
        revision_id(),
        ManifestGeneration::new(1),
        "manifest:cluster",
    )
    .source_event(sample_source_event())
    .replace_semantic_scope(
        sample_cluster_semantic_scope("auth-service"),
        "scope:cluster",
        vec![source_b, source_a],
        vec![membership_b, membership_a.clone()],
    );

    let _receipt = ok_or_fail!(client.search_corpus().publish(&batch));
    let captured = ok_or_fail!(only_ingest_request(ingest.as_ref()));
    let SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(wire) = captured.payload else {
        panic!("expected search corpus wire batch");
    };
    let Some(scope) = wire.semantic_replace_scopes.first() else {
        panic!("expected one semantic replace scope");
    };
    assert_eq!(scope.cluster_memberships.len(), 2);
    assert_eq!(scope.cluster_memberships.first(), Some(&membership_a));
    assert!(
        scope
            .cluster_memberships
            .iter()
            .flat_map(|membership| membership.members.iter())
            .all(|member| member.as_str() != "symbol:fake"),
        "rendered source text must never synthesize structured membership"
    );
}

#[test]
fn search_corpus_builder_rejects_missing_mismatched_or_misplaced_cluster_membership_v1() {
    let ingest = Arc::new(StubIngestTransport::new(unused_ingest_response()));
    let client = QuantaIndex::from_transports(unused_query(), unused_control(), ingest.clone());
    let cluster_source = sample_cluster_semantic_source("auth-service", "a");

    let missing = SearchCorpusBatch::replace_generation(
        repo_id(),
        revision_id(),
        ManifestGeneration::new(1),
        "manifest:cluster-missing",
    )
    .source_event(sample_source_event())
    .replace_semantic_scope(
        sample_cluster_semantic_scope("auth-service"),
        "scope:cluster-missing",
        vec![cluster_source.clone()],
        Vec::new(),
    );
    let error = client
        .search_corpus()
        .publish(&missing)
        .expect_err("ClusterCard without typed membership must fail closed");
    assert!(error.to_string().contains("requires one typed membership"));

    let mut stale_membership = sample_cluster_membership(&cluster_source, "a");
    stale_membership.authority_digest = "stale-authority".to_string();
    let mismatched = SearchCorpusBatch::replace_generation(
        repo_id(),
        revision_id(),
        ManifestGeneration::new(1),
        "manifest:cluster-mismatch",
    )
    .source_event(sample_source_event())
    .replace_semantic_scope(
        sample_cluster_semantic_scope("auth-service"),
        "scope:cluster-mismatch",
        vec![cluster_source],
        vec![stale_membership],
    );
    let error = client
        .search_corpus()
        .publish(&mismatched)
        .expect_err("stale typed membership authority must fail closed");
    assert!(error.to_string().contains("digest does not match"));

    let symbol_source = sample_semantic_source("symbol-a");
    let misplaced = SearchCorpusBatch::replace_generation(
        repo_id(),
        revision_id(),
        ManifestGeneration::new(1),
        "manifest:membership-misplaced",
    )
    .source_event(sample_source_event())
    .replace_semantic_scope(
        sample_semantic_scope("symbol-a"),
        "scope:membership-misplaced",
        vec![symbol_source.clone()],
        vec![sample_cluster_membership(&symbol_source, "misplaced")],
    );
    let error = client
        .search_corpus()
        .publish(&misplaced)
        .expect_err("non-ClusterCard scope with typed membership must fail closed");
    assert!(
        error
            .to_string()
            .contains("must not carry cluster membership")
    );

    assert!(
        ok_or_fail!(ingest.requests.lock()).is_empty(),
        "invalid cluster membership authority must be rejected before transport"
    );
}

#[test]
fn search_corpus_semantic_surface_conflict_fails_before_transport_io() {
    let ingest = Arc::new(StubIngestTransport::new(unused_ingest_response()));
    let client = QuantaIndex::from_transports(unused_query(), unused_control(), ingest.clone());
    let batch = SearchCorpusBatch::replace_generation(
        repo_id(),
        revision_id(),
        ManifestGeneration::new(1),
        "manifest:conflict",
    )
    .source_event(sample_source_event())
    .clear_surface(SearchScopeSurface::Symbol)
    .replace_semantic_scope(
        sample_semantic_scope("symbol-conflict"),
        "scope:conflict",
        vec![sample_semantic_source("symbol-conflict")],
        Vec::new(),
    );

    let error = client
        .search_corpus()
        .publish(&batch)
        .expect_err("clear plus semantic replace must fail closed");
    assert!(error.to_string().contains("cannot be cleared and replaced"));
    let requests_are_empty = {
        let requests = ingest
            .requests
            .lock()
            .expect("ingest request list should remain readable");
        requests.is_empty()
    };
    assert!(requests_are_empty, "invalid batch must not reach transport");
}

#[test]
fn search_corpus_semantic_scope_conflicts_fail_before_transport_io() {
    let ingest = Arc::new(StubIngestTransport::new(unused_ingest_response()));
    let client = QuantaIndex::from_transports(unused_query(), unused_control(), ingest.clone());
    let scope = sample_semantic_scope("symbol-conflict");
    let replace_and_tombstone = SearchCorpusBatch::replace_generation(
        repo_id(),
        revision_id(),
        ManifestGeneration::new(1),
        "manifest:scope-conflict",
    )
    .source_event(sample_source_event())
    .replace_semantic_scope(
        scope.clone(),
        "scope:conflict",
        vec![sample_semantic_source("symbol-conflict")],
        Vec::new(),
    )
    .tombstone_semantic_scope(scope.clone());

    let error = client
        .search_corpus()
        .publish(&replace_and_tombstone)
        .expect_err("same semantic scope replace plus tombstone must fail closed");
    assert!(
        error
            .to_string()
            .contains("cannot be replaced and tombstoned")
    );

    let duplicate_replace = SearchCorpusBatch::replace_generation(
        repo_id(),
        revision_id(),
        ManifestGeneration::new(1),
        "manifest:duplicate-scope",
    )
    .source_event(sample_source_event())
    .replace_semantic_scope(
        scope.clone(),
        "scope:first",
        vec![sample_semantic_source("symbol-conflict")],
        Vec::new(),
    )
    .replace_semantic_scope(
        scope,
        "scope:second",
        vec![sample_semantic_source("symbol-conflict")],
        Vec::new(),
    );
    let error = client
        .search_corpus()
        .publish(&duplicate_replace)
        .expect_err("duplicate semantic replace scope must fail closed");
    assert!(error.to_string().contains("duplicate replace scope"));

    let requests_are_empty = ingest
        .requests
        .lock()
        .expect("ingest request list should remain readable")
        .is_empty();
    assert!(
        requests_are_empty,
        "invalid batches must not reach transport"
    );
}

#[test]
fn reader_client_routes_lexical_query_surface() {
    let query = Arc::new(StubQueryTransport::active(
        SearchPlaneQueryIpcResponse::Text(TextQueryResponse {
            selected_active_head: None,
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
    let response = ok_or_fail!(
        client
            .reader()
            .lexical()
            .native("reader needle")
            .active(repo_id(), revision_id())
            .top_k(4)
            .execute()
    );
    assert_eq!(response.generation, sample_generation_pin());
    let captured = ok_or_fail!(only_query_request(query.as_ref()));
    assert!(
        matches!(
            captured.payload,
            quanta_index_contract::SearchPlaneQueryIpcRequest::Text(_)
        ),
        "expected text request, got {:?}",
        captured.payload
    );
    let quanta_index_contract::SearchPlaneQueryIpcRequest::Text(req) = &captured.payload else {
        return;
    };
    assert_eq!(req.query_text, "reader needle");
    assert_eq!(req.top_k, 4);
}

#[test]
fn producer_client_refuses_missing_source_event_before_transport() {
    let ingest = unused_ingest();
    let client = QuantaIndex::from_transports(unused_query(), unused_control(), ingest.clone());
    let batch = SearchCorpusBatch::replace_generation(
        repo_id(),
        revision_id(),
        ManifestGeneration::new(2),
        "manifest:no-event",
    );
    let error = client
        .producer()
        .publish_search_corpus(&batch)
        .expect_err("generation identity cannot replace producer event identity");
    assert!(matches!(error, crate::SdkError::Serialization(ref message)
        if message.contains("source-event identity is required")));
    assert!(
        ingest
            .requests
            .lock()
            .expect("ingest request mutex")
            .is_empty()
    );
}

#[test]
fn producer_client_publish_search_corpus_refuses_unsealed_before_transport() {
    let ingest = Arc::new(StubIngestTransport::for_corpus_receipt(
        BatchPublishReceipt {
            generation: ManifestGeneration::new(2),
            manifest_digest: Some("manifest:unsealed".to_string()),
            batch_digest: String::new(),
            applied: true,
            durable_sequence: 7,
            semantic_content: None,
            accepted_clear_surfaces: 0,
            accepted_replace_scopes: 0,
            accepted_tombstone_scopes: 0,
            accepted_semantic_replace_scopes: 0,
            accepted_semantic_tombstone_scopes: 0,
            sealed: false,
        },
    ));
    let client = QuantaIndex::from_transports(unused_query(), unused_control(), ingest.clone());
    let batch = SearchCorpusBatch::replace_generation(
        repo_id(),
        revision_id(),
        ManifestGeneration::new(2),
        "manifest:unsealed",
    )
    .source_event(sample_source_event())
    .without_seal();
    let error = client
        .producer()
        .publish_search_corpus(&batch)
        .expect_err("source-event publication requires a sealed batch");
    assert!(matches!(error, crate::SdkError::Serialization(ref message)
        if message.contains("requires a sealed batch")));
    assert!(
        ingest
            .requests
            .lock()
            .expect("ingest request mutex")
            .is_empty()
    );
}

struct ObservedIngestTransport {
    mutation: Option<&'static str>,
}

impl IngestTransport for ObservedIngestTransport {
    fn send(
        &self,
        request: SearchPlaneIngestIpcRequestEnvelope,
    ) -> Result<SearchPlaneIngestIpcResponseEnvelope, crate::SdkError> {
        use quanta_index_contract::{
            IngestObservationStatus, IngestStageReport, SearchCorpusIngestObservation,
            SearchCorpusPublishOutcome,
        };
        let SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(batch) = request.payload else {
            return Err(crate::SdkError::Protocol(
                "expected corpus publish".to_string(),
            ));
        };
        let receipt = BatchPublishReceipt {
            sealed: true,
            ..BatchPublishReceipt::empty_for(
                batch.generation,
                Some(batch.manifest_digest.clone()),
                batch.batch_digest.clone(),
            )
        };
        let publication = quanta_index_contract::SourcePublicationBinding::for_batch(&batch);
        let mut observation = SearchCorpusIngestObservation {
            request_id: request.request_id,
            repo_id: batch.repo_id,
            revision_id: batch.revision_id,
            generation: batch.generation,
            batch_digest: batch.batch_digest,
            status: IngestObservationStatus::Executed,
            semantic: Some(Box::new(IngestStageReport {
                durations: quanta_index_contract::IngestStageDurations {
                    total: 11,
                    seal: Some(11),
                    ..Default::default()
                },
                ..Default::default()
            })),
            lexical_build_ns: Some(17),
            lexical_stages: None,
            finalize_ns: Some(23),
            activation_ns: None,
        };
        match self.mutation {
            Some("request") => {
                observation.request_id =
                    observation.request_id.checked_add(1).ok_or_else(|| {
                        crate::SdkError::Protocol("fixture request id overflow".to_string())
                    })?;
            }
            Some("repo") => {
                observation.repo_id = RepoId::new("other-repo")
                    .map_err(|error| crate::SdkError::Protocol(error.to_string()))?;
            }
            Some("revision") => {
                observation.revision_id = RevisionId::new("other-revision")
                    .map_err(|error| crate::SdkError::Protocol(error.to_string()))?;
            }
            Some("generation") => observation.generation = ManifestGeneration::new(99),
            Some("batch") => observation.batch_digest = "other-batch".to_string(),
            Some("replay") => observation.status = IngestObservationStatus::Replayed,
            Some("activation") => observation.activation_ns = Some(1),
            Some("seal") => {
                observation
                    .semantic
                    .as_mut()
                    .expect("fixture semantic stage")
                    .durations
                    .seal = None;
            }
            Some("missing") | None => {}
            Some(other) => {
                return Err(crate::SdkError::Protocol(format!("unknown mutant {other}")));
            }
        }
        Ok(SearchPlaneIngestIpcResponseEnvelope {
            request_id: request.request_id,
            payload: SearchPlaneIngestIpcResponse::SearchCorpusReceipt(
                SearchCorpusPublishOutcome {
                    publication,
                    receipt,
                    observation: if self.mutation == Some("missing") {
                        None
                    } else {
                        Some(observation)
                    },
                },
            ),
        })
    }
}

#[test]
fn observed_corpus_publish_propagates_typed_stages_and_rejects_identity_and_replay_forgery() {
    let batch = SearchCorpusBatch::replace_generation(
        repo_id(),
        revision_id(),
        ManifestGeneration::new(2),
        "manifest:observed",
    )
    .source_event(sample_source_event());
    let client = QuantaIndex::from_transports(
        unused_query(),
        unused_control(),
        Arc::new(ObservedIngestTransport { mutation: None }),
    );
    let outcome = ok_or_fail!(client.producer().publish_search_corpus_observed(&batch));
    let observation = outcome.observation.expect("fixture has observed stages");
    assert_eq!(observation.lexical_build_ns, Some(17));
    assert_eq!(observation.finalize_ns, Some(23));
    assert_eq!(observation.activation_ns, None);
    for mutation in [
        "request",
        "repo",
        "revision",
        "generation",
        "batch",
        "replay",
        "activation",
        "seal",
        "missing",
    ] {
        let client = QuantaIndex::from_transports(
            unused_query(),
            unused_control(),
            Arc::new(ObservedIngestTransport {
                mutation: Some(mutation),
            }),
        );
        assert!(
            client
                .producer()
                .publish_search_corpus_observed(&batch)
                .is_err(),
            "accepted invalid observation {mutation}"
        );
    }
}

#[test]
fn producer_client_publish_search_corpus_and_activate_routes_ingest_then_control() {
    let receipt = BatchPublishReceipt {
        generation: ManifestGeneration::new(7),
        manifest_digest: Some("manifest:activate".to_string()),
        batch_digest: String::new(),
        applied: true,
        durable_sequence: 7,
        semantic_content: Some(semantic_roots(7)),
        accepted_clear_surfaces: 0,
        accepted_replace_scopes: 0,
        accepted_tombstone_scopes: 0,
        accepted_semantic_replace_scopes: 0,
        accepted_semantic_tombstone_scopes: 0,
        sealed: true,
    };
    let manifest_digest = receipt
        .manifest_digest
        .clone()
        .expect("search corpus receipt carries its manifest digest");
    let active = search_corpus_identity(receipt.generation.get(), &manifest_digest);
    let ack = SearchPlaneSearchCorpusActivationCasAck {
        active: head_with_generation(active.clone(), 1),
        previous_sealed_active: None,
    };
    let control = Arc::new(StubControlTransport::new(
        quanta_index_contract::SearchPlaneControlIpcResponse::SearchCorpusActivationCasAck(
            ack.clone(),
        ),
    ));
    let ingest = Arc::new(StubIngestTransport::for_corpus_receipt(receipt.clone()));
    let client = QuantaIndex::from_transports(unused_query(), control.clone(), ingest.clone());
    let batch = SearchCorpusBatch::replace_generation(
        repo_id(),
        revision_id(),
        ManifestGeneration::new(7),
        "manifest:activate",
    )
    .source_event(sample_source_event());
    let (observed_receipt, observed_ack) = ok_or_fail!(
        client
            .producer()
            .publish_search_corpus_and_activate(&batch, None)
    );
    let expected_receipt = BatchPublishReceipt {
        batch_digest: ok_or_fail!(batch.batch_digest()),
        ..receipt
    };
    assert_eq!(observed_receipt, expected_receipt);
    assert_eq!(observed_ack, ack);

    let ingest_request = ok_or_fail!(only_ingest_request(ingest.as_ref()));
    assert!(matches!(
        ingest_request.payload,
        SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(_)
    ));
    let control_request = ok_or_fail!(only_control_request(control.as_ref()));
    let quanta_index_contract::SearchPlaneControlIpcRequest::ActivateSearchCorpusGenerationCas(
        request,
    ) = &control_request.payload
    else {
        panic!("expected composite search corpus activation CAS request");
    };
    assert_eq!(request.candidate, active);
    assert_eq!(request.expected_active, None);
}

#[test]
fn producer_client_rejects_activation_ack_identity_mismatches_v1() {
    let candidate = search_corpus_identity(7, "manifest:activate");
    let previous = search_corpus_head(6, "manifest:previous", 1);
    let mut wrong_repo = candidate.clone();
    wrong_repo.lexical.repo_id =
        RepoId::new("other-repo").expect("static fixture ID satisfies canonical policy");
    let mut wrong_revision = candidate.clone();
    wrong_revision.semantic.revision_id =
        RevisionId::new("other-revision").expect("static fixture ID satisfies canonical policy");
    let wrong_generation = search_corpus_identity(8, "manifest:activate");
    let wrong_digest = search_corpus_identity(7, "manifest:other");
    // Same tracks, other semantic content roots: a different identity
    // (QI-BB-028), refused like every other ack drift.
    let mut wrong_roots = candidate.clone();
    wrong_roots.semantic_content = semantic_roots(70);
    let cases = [
        (
            "active semantic content roots",
            SearchPlaneSearchCorpusActivationCasAck {
                active: head_with_generation(wrong_roots, 2),
                previous_sealed_active: Some(previous.clone()),
            },
        ),
        (
            "active repo",
            SearchPlaneSearchCorpusActivationCasAck {
                active: head_with_generation(wrong_repo, 2),
                previous_sealed_active: Some(previous.clone()),
            },
        ),
        (
            "active revision",
            SearchPlaneSearchCorpusActivationCasAck {
                active: head_with_generation(wrong_revision, 2),
                previous_sealed_active: Some(previous.clone()),
            },
        ),
        (
            "active generation",
            SearchPlaneSearchCorpusActivationCasAck {
                active: head_with_generation(wrong_generation, 2),
                previous_sealed_active: Some(previous.clone()),
            },
        ),
        (
            "active digest",
            SearchPlaneSearchCorpusActivationCasAck {
                active: head_with_generation(wrong_digest, 2),
                previous_sealed_active: Some(previous.clone()),
            },
        ),
        (
            "missing previous",
            SearchPlaneSearchCorpusActivationCasAck {
                active: head_with_generation(candidate.clone(), 2),
                previous_sealed_active: None,
            },
        ),
        (
            "wrong previous",
            SearchPlaneSearchCorpusActivationCasAck {
                active: head_with_generation(candidate, 2),
                previous_sealed_active: Some(search_corpus_head(5, "manifest:older", 1)),
            },
        ),
    ];

    for (label, ack) in cases {
        let control = Arc::new(StubControlTransport::new(
            quanta_index_contract::SearchPlaneControlIpcResponse::SearchCorpusActivationCasAck(ack),
        ));
        let ingest = Arc::new(StubIngestTransport::for_corpus_receipt(
            BatchPublishReceipt {
                generation: ManifestGeneration::new(7),
                manifest_digest: Some("manifest:activate".to_string()),
                batch_digest: String::new(),
                applied: true,
                durable_sequence: 7,
                semantic_content: Some(semantic_roots(7)),
                accepted_clear_surfaces: 0,
                accepted_replace_scopes: 0,
                accepted_tombstone_scopes: 0,
                accepted_semantic_replace_scopes: 0,
                accepted_semantic_tombstone_scopes: 0,
                sealed: true,
            },
        ));
        let client = QuantaIndex::from_transports(unused_query(), control, ingest);
        let batch = SearchCorpusBatch::replace_generation(
            repo_id(),
            revision_id(),
            ManifestGeneration::new(7),
            "manifest:activate",
        )
        .source_event(sample_source_event());
        let error = client
            .producer()
            .publish_search_corpus_and_activate(&batch, Some(previous.clone()))
            .expect_err(label);
        assert!(
            matches!(
                error,
                crate::SdkError::Binding {
                    axis: crate::ResponseBindingAxis::TargetIdentity,
                    ..
                } | crate::SdkError::Binding {
                    axis: crate::ResponseBindingAxis::CasExpectation,
                    ..
                }
            ) || matches!(error, crate::SdkError::Protocol(ref message) if message.contains("acknowledgement")),
            "{label} mismatch must fail closed, got {error:?}"
        );
    }
}

/// A sealed receipt that attests no semantic content roots cannot become
/// an activation candidate (QI-BB-028): the SDK refuses before any control
/// request rather than naming roots it does not know.
#[test]
fn producer_client_refuses_to_activate_on_a_sealed_receipt_without_content_roots_v1() {
    let receipt = BatchPublishReceipt {
        generation: ManifestGeneration::new(7),
        manifest_digest: Some("manifest:activate".to_string()),
        batch_digest: "batch:activate".to_string(),
        applied: true,
        durable_sequence: 7,
        semantic_content: None,
        accepted_clear_surfaces: 0,
        accepted_replace_scopes: 0,
        accepted_tombstone_scopes: 0,
        accepted_semantic_replace_scopes: 0,
        accepted_semantic_tombstone_scopes: 0,
        sealed: true,
    };
    let control = unused_control();
    let ingest = Arc::new(StubIngestTransport::for_corpus_receipt(receipt));
    let client = QuantaIndex::from_transports(unused_query(), control.clone(), ingest);
    let batch = SearchCorpusBatch::replace_generation(
        repo_id(),
        revision_id(),
        ManifestGeneration::new(7),
        "manifest:activate",
    )
    .source_event(sample_source_event());
    let error = client
        .producer()
        .publish_search_corpus_and_activate(&batch, None)
        .expect_err("a receipt without content roots must not be activated");
    assert!(
        matches!(error, crate::SdkError::Protocol(ref message) if message.contains("attests no semantic content roots")),
        "got {error:?}"
    );
    assert!(
        control
            .requests
            .lock()
            .expect("control request mutex")
            .is_empty(),
        "no control request is sent without roots to name"
    );
}

#[test]
fn producer_client_rejects_mismatched_sealed_receipt_before_composite_activation_v1() {
    let receipt = BatchPublishReceipt {
        generation: ManifestGeneration::new(7),
        manifest_digest: Some("manifest:unexpected".to_string()),
        batch_digest: String::new(),
        applied: true,
        durable_sequence: 7,
        semantic_content: None,
        accepted_clear_surfaces: 0,
        accepted_replace_scopes: 0,
        accepted_tombstone_scopes: 0,
        accepted_semantic_replace_scopes: 0,
        accepted_semantic_tombstone_scopes: 0,
        sealed: true,
    };
    let control = unused_control();
    let ingest = Arc::new(StubIngestTransport::for_corpus_receipt(receipt));
    let client = QuantaIndex::from_transports(unused_query(), control.clone(), ingest);
    let batch = SearchCorpusBatch::replace_generation(
        repo_id(),
        revision_id(),
        ManifestGeneration::new(7),
        "manifest:expected",
    )
    .source_event(sample_source_event());
    let error = client
        .producer()
        .publish_search_corpus_and_activate(&batch, None)
        .expect_err("mismatched sealed receipt must not be activated");
    assert!(
        matches!(
            error,
            crate::SdkError::Binding {
                axis: crate::ResponseBindingAxis::BatchCommitment,
                ..
            }
        ),
        "expected fail-closed receipt-integrity error, got {error:?}"
    );
    assert!(
        control
            .requests
            .lock()
            .expect("control request mutex")
            .is_empty(),
        "receipt mismatch must not emit a composite activation request"
    );
}

#[test]
fn producer_client_rejects_each_search_corpus_receipt_mismatch_before_activation_v1() {
    let batch = SearchCorpusBatch::replace_generation(
        repo_id(),
        revision_id(),
        ManifestGeneration::new(7),
        "manifest:receipt-exact",
    )
    .source_event(sample_source_event())
    .replace_scope(
        sample_source_coverage(),
        b"fn sample() {}".to_vec(),
        vec![sample_chunk()],
        vec![sample_symbol()],
    )
    .tombstone_scope(quanta_index_contract::SourceFileKey {
        source_repo_id: repo_id(),
        repo_relative_path: RepoRelativePath::new("src/tombstone.rs"),
    });
    let valid = BatchPublishReceipt {
        generation: ManifestGeneration::new(7),
        manifest_digest: Some("manifest:receipt-exact".to_string()),
        batch_digest: ok_or_fail!(batch.batch_digest()),
        applied: true,
        durable_sequence: 7,
        semantic_content: None,
        accepted_clear_surfaces: 0,
        accepted_replace_scopes: 1,
        accepted_tombstone_scopes: 1,
        accepted_semantic_replace_scopes: 0,
        accepted_semantic_tombstone_scopes: 0,
        sealed: true,
    };
    let mut cases = Vec::new();

    let mut wrong_generation = valid.clone();
    wrong_generation.generation = ManifestGeneration::new(8);
    cases.push(("generation", wrong_generation));

    let mut wrong_digest = valid.clone();
    wrong_digest.manifest_digest = Some("manifest:other".to_string());
    cases.push(("manifest digest", wrong_digest));

    let mut wrong_batch_digest = valid.clone();
    wrong_batch_digest.batch_digest = "batch:other".to_string();
    cases.push(("batch digest", wrong_batch_digest));

    let mut wrong_seal = valid.clone();
    wrong_seal.sealed = false;
    cases.push(("seal", wrong_seal));

    let mut wrong_replace_count = valid.clone();
    wrong_replace_count.accepted_replace_scopes = 0;
    cases.push(("replace scope", wrong_replace_count));

    let mut wrong_tombstone_count = valid.clone();
    wrong_tombstone_count.accepted_tombstone_scopes = 0;
    cases.push(("tombstone scope", wrong_tombstone_count));

    let mut wrong_semantic_replace_count = valid.clone();
    wrong_semantic_replace_count.accepted_semantic_replace_scopes = 1;
    cases.push(("semantic replace scope", wrong_semantic_replace_count));

    let mut wrong_semantic_tombstone_count = valid.clone();
    wrong_semantic_tombstone_count.accepted_semantic_tombstone_scopes = 1;
    cases.push(("semantic tombstone scope", wrong_semantic_tombstone_count));

    let mut wrong_clear_count = valid;
    wrong_clear_count.accepted_clear_surfaces = 1;
    cases.push(("clear surface", wrong_clear_count));

    for (label, receipt) in cases {
        let control = unused_control();
        let ingest = Arc::new(StubIngestTransport::answering_verbatim(
            SearchPlaneIngestIpcResponse::SearchCorpusReceipt(
                quanta_index_contract::SearchCorpusPublishOutcome {
                    publication: quanta_index_contract::SourcePublicationBinding::for_batch(
                        &ok_or_fail!(batch.to_wire_batch()),
                    ),
                    receipt,
                    observation: None,
                },
            ),
        ));
        let client = QuantaIndex::from_transports(unused_query(), control.clone(), ingest);
        let error = client
            .producer()
            .publish_search_corpus_and_activate(&batch, None)
            .expect_err(label);
        assert!(
            matches!(
                error,
                crate::SdkError::Binding {
                    axis: crate::ResponseBindingAxis::BatchCommitment,
                    ..
                }
            ) || matches!(error, crate::SdkError::Protocol(ref message) if message.contains(label)),
            "{label} mismatch must fail closed, got {error:?}"
        );
        assert!(
            control
                .requests
                .lock()
                .expect("control request mutex")
                .is_empty(),
            "{label} mismatch emitted a control request"
        );
    }
}

#[test]
fn producer_client_rejects_invalid_expected_composite_before_ingest_v1() {
    let invalid_expected = SearchCorpusGenerationIdentityV1 {
        lexical: GenerationSnapshot {
            repo_id: repo_id(),
            revision_id: revision_id(),
            track: Track::Lexical,
            manifest_generation: ManifestGeneration::new(6),
            manifest_digest: "manifest:6".to_string(),
        },
        semantic: GenerationSnapshot {
            repo_id: repo_id(),
            revision_id: revision_id(),
            track: Track::Lexical,
            manifest_generation: ManifestGeneration::new(6),
            manifest_digest: "manifest:6".to_string(),
        },
        semantic_content: semantic_roots(6),
    };
    let ingest = unused_ingest();
    let client = QuantaIndex::from_transports(unused_query(), unused_control(), ingest.clone());
    let batch = SearchCorpusBatch::replace_generation(
        repo_id(),
        revision_id(),
        ManifestGeneration::new(7),
        "manifest:7",
    )
    .source_event(sample_source_event());
    let error = client
        .producer()
        .publish_search_corpus_and_activate(&batch, Some(head_with_generation(invalid_expected, 1)))
        .expect_err("lexical-only expected identity must be rejected before ingest");
    assert!(
        matches!(error, crate::SdkError::Protocol(ref message) if message.contains("SEMANTIC_TRACK_REQUIRED")),
        "expected typed composite-identity error, got {error:?}"
    );
    assert!(
        ingest
            .requests
            .lock()
            .expect("ingest request mutex")
            .is_empty(),
        "invalid expected identity must not seal or publish a batch"
    );
}

#[test]
fn producer_client_delegates_non_advancing_activation_rejection_before_ingest_v1() {
    let ingest = unused_ingest();
    let client = QuantaIndex::from_transports(unused_query(), unused_control(), ingest.clone());
    let batch = SearchCorpusBatch::replace_generation(
        repo_id(),
        revision_id(),
        ManifestGeneration::new(7),
        "manifest:7-new",
    )
    .source_event(sample_source_event());
    let error = client
        .producer()
        .publish_search_corpus_and_activate(
            &batch,
            Some(search_corpus_head(7, "manifest:7-current", 1)),
        )
        .expect_err("activation candidate must strictly advance the expected active generation");
    assert!(
        matches!(error, crate::SdkError::Protocol(ref message) if message.contains("CANDIDATE_GENERATION_MUST_ADVANCE_EXPECTED_ACTIVE")),
        "expected contract-owned generation relation error, got {error:?}"
    );
    assert!(
        ingest
            .requests
            .lock()
            .expect("ingest request mutex")
            .is_empty(),
        "non-advancing activation reached ingest"
    );
}

struct MultipartCorpusTransport {
    body: Vec<u8>,
    identity: quanta_index_contract::SourcePublicationUploadIdentity,
    publication: quanta_index_contract::SourcePublicationBinding,
    receipt: BatchPublishReceipt,
    progress: Mutex<(usize, usize, usize)>,
}

impl IngestTransport for MultipartCorpusTransport {
    fn send(
        &self,
        request: SearchPlaneIngestIpcRequestEnvelope,
    ) -> Result<SearchPlaneIngestIpcResponseEnvelope, crate::SdkError> {
        let payload = match request.payload {
            SearchPlaneIngestIpcRequest::StageSourcePublication(part) => {
                let next_offset = {
                    let mut progress = self
                        .progress
                        .lock()
                        .map_err(|error| crate::SdkError::Protocol(error.to_string()))?;
                    let offset = u64::try_from(progress.0)
                        .map_err(|error| crate::SdkError::Protocol(error.to_string()))?;
                    if part.identity != self.identity || part.offset != offset {
                        return Err(crate::SdkError::Protocol(
                            "multipart fixture received a different identity or offset".into(),
                        ));
                    }
                    if part.bytes.len() > 1_048_576 {
                        return Err(crate::SdkError::Protocol(
                            "multipart fixture received an oversized part".into(),
                        ));
                    }
                    let end = progress.0.checked_add(part.bytes.len()).ok_or_else(|| {
                        crate::SdkError::Protocol("multipart fixture part end overflowed".into())
                    })?;
                    if self.body.get(progress.0..end) != Some(part.bytes.as_slice()) {
                        return Err(crate::SdkError::Protocol(
                            "multipart fixture received different body bytes".into(),
                        ));
                    }
                    let part_count = progress.1.checked_add(1).ok_or_else(|| {
                        crate::SdkError::Protocol("multipart fixture part count overflowed".into())
                    })?;
                    let next_offset = u64::try_from(end)
                        .map_err(|error| crate::SdkError::Protocol(error.to_string()))?;
                    progress.0 = end;
                    progress.1 = part_count;
                    next_offset
                };
                SearchPlaneIngestIpcResponse::SourcePublicationUploadAck(
                    quanta_index_contract::SourcePublicationUploadAck {
                        identity: self.identity,
                        next_offset,
                    },
                )
            }
            SearchPlaneIngestIpcRequest::PublishStagedSourcePublication(commit) => {
                {
                    let mut progress = self
                        .progress
                        .lock()
                        .map_err(|error| crate::SdkError::Protocol(error.to_string()))?;
                    if progress.0 != self.body.len()
                        || commit.identity != self.identity
                        || commit.publication != self.publication
                    {
                        return Err(crate::SdkError::Protocol(
                            "multipart fixture received an incomplete or different commit".into(),
                        ));
                    }
                    progress.2 = progress.2.checked_add(1).ok_or_else(|| {
                        crate::SdkError::Protocol(
                            "multipart fixture commit count overflowed".into(),
                        )
                    })?;
                }
                SearchPlaneIngestIpcResponse::SearchCorpusReceipt(
                    quanta_index_contract::SearchCorpusPublishOutcome {
                        publication: self.publication.clone(),
                        receipt: self.receipt.clone(),
                        observation: None,
                    },
                )
            }
            SearchPlaneIngestIpcRequest::DiscardSourcePublicationUpload(_)
            | SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(_)
            | SearchPlaneIngestIpcRequest::PublishHistoryBatch(_)
            | SearchPlaneIngestIpcRequest::PublishRepoCommitRecencyBatch(_)
            | SearchPlaneIngestIpcRequest::PublishRepoTopicBatch(_)
            | SearchPlaneIngestIpcRequest::PublishFileOwnershipBatch(_)
            | SearchPlaneIngestIpcRequest::PublishFileContributorBatch(_)
            | SearchPlaneIngestIpcRequest::PublishDirtyBatch(_)
            | SearchPlaneIngestIpcRequest::PublishRuntimeCatalogBatch(_)
            | SearchPlaneIngestIpcRequest::PublishStructuralBatch(_)
            | SearchPlaneIngestIpcRequest::PublishRepoMapBundleV2(_)
            | SearchPlaneIngestIpcRequest::PublishRepoMetaBatch(_)
            | SearchPlaneIngestIpcRequest::PublishRepoDescriptionBatch(_) => {
                return Err(crate::SdkError::Protocol(
                    "large SDK publication used a different route".into(),
                ));
            }
        };
        Ok(SearchPlaneIngestIpcResponseEnvelope {
            request_id: request.request_id,
            payload,
        })
    }
}

#[test]
fn large_sdk_publication_streams_exact_original_body_then_commits_once() {
    let mut source = sample_semantic_source("large-symbol");
    source.text = "x".repeat(65 * 1024 * 1024);
    let batch = SearchCorpusBatch::replace_generation(
        repo_id(),
        revision_id(),
        ManifestGeneration::new(1),
        "manifest:multipart",
    )
    .source_event(sample_source_event())
    .replace_semantic_scope(
        sample_semantic_scope("large-symbol"),
        "scope:multipart",
        vec![source],
        vec![],
    );
    let wire = ok_or_fail!(batch.to_wire_batch());
    // Independent whole-body encoding is a test oracle, never the production upload path.
    let body = ok_or_fail!(quanta_index_ipc::encode_cbor_payload(&wire));
    assert!(body.len() > 64 * 1024 * 1024);
    let transport = Arc::new(MultipartCorpusTransport {
        body,
        identity: ok_or_fail!(quanta_index_ipc::source_publication_upload_identity(&wire)),
        publication: quanta_index_contract::SourcePublicationBinding::for_batch(&wire),
        receipt: BatchPublishReceipt {
            generation: ManifestGeneration::new(1),
            manifest_digest: Some("manifest:multipart".into()),
            batch_digest: wire.batch_digest.clone(),
            applied: true,
            durable_sequence: 7,
            semantic_content: None,
            accepted_clear_surfaces: 0,
            accepted_replace_scopes: 0,
            accepted_tombstone_scopes: 0,
            accepted_semantic_replace_scopes: 1,
            accepted_semantic_tombstone_scopes: 0,
            sealed: true,
        },
        progress: Mutex::new((0, 0, 0)),
    });
    let client = QuantaIndex::from_transports(unused_query(), unused_control(), transport.clone());
    assert_eq!(
        ok_or_fail!(client.search_corpus().publish(&batch)),
        transport.receipt
    );
    let progress = *transport.progress.lock().expect("progress");
    assert_eq!(progress.0, transport.body.len());
    assert!(progress.1 > 64);
    assert_eq!(progress.2, 1);
}
