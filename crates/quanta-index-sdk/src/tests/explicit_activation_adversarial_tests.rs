use super::*;
use quanta_index_contract::SearchPlaneControlIpcResponse;

fn original_evidence() -> (SearchCorpusBatch, crate::PublishedBatchEvidence) {
    let batch = SearchCorpusBatch::replace_generation(
        repo_id(),
        revision_id(),
        ManifestGeneration::new(7),
        "manifest:original",
    )
    .source_event(sample_source_event());
    let wire = ok_or_fail!(batch.to_wire_batch());
    let evidence = crate::PublishedBatchEvidence {
        publication: quanta_index_contract::SourcePublicationBinding::for_batch(&wire),
        receipt: BatchPublishReceipt {
            sealed: true,
            applied: false,
            durable_sequence: 17,
            semantic_content: Some(semantic_roots(7)),
            ..BatchPublishReceipt::empty_for(
                wire.generation,
                Some(wire.manifest_digest),
                wire.batch_digest,
            )
        },
    };
    (batch, evidence)
}

#[test]
fn explicit_replay_activates_original_revision_without_observation_or_second_ingest() {
    let (_original, evidence) = original_evidence();
    let retargeted = SearchCorpusBatch::replace_generation(
        repo_id(),
        ok_or_fail!(RevisionId::new("submitted-revision")),
        ManifestGeneration::new(99),
        "manifest:submitted",
    )
    .source_event(sample_source_event());
    let outcome = quanta_index_contract::SearchCorpusPublishOutcome {
        publication: evidence.publication.clone(),
        receipt: evidence.receipt.clone(),
        observation: None,
    };
    let ingest = Arc::new(StubIngestTransport::answering_verbatim(
        SearchPlaneIngestIpcResponse::SearchCorpusReceipt(outcome),
    ));
    let original_identity = search_corpus_identity(7, "manifest:original");
    let ack = SearchPlaneSearchCorpusActivationCasAck {
        active: head_with_generation(original_identity.clone(), 1),
        previous_sealed_active: None,
    };
    let control = Arc::new(StubControlTransport::new(
        SearchPlaneControlIpcResponse::SearchCorpusActivationCasAck(ack.clone()),
    ));
    let query = unused_query();
    let client = QuantaIndex::from_transports(query.clone(), control.clone(), ingest.clone());

    let published = ok_or_fail!(client.producer().publish_search_corpus_outcome(&retargeted));
    assert!(published.observation.is_none());
    assert_eq!(published.publication, evidence.publication);
    assert_eq!(published.receipt, evidence.receipt);
    assert_ne!(
        published.publication.target.revision_id,
        retargeted.revision_id().clone()
    );
    assert_eq!(
        ok_or_fail!(client.producer().activate_published_search_corpus(
            &crate::PublishedBatchEvidence::from(&published),
            None,
        )),
        ack
    );
    assert_eq!(ok_or_fail!(ingest.requests.lock()).len(), 1);
    assert!(ok_or_fail!(query.requests.lock()).is_empty());
    let request = ok_or_fail!(only_control_request(&control));
    let quanta_index_contract::SearchPlaneControlIpcRequest::ActivateSearchCorpusGenerationCas(cas) =
        request.payload
    else {
        panic!("expected explicit activation CAS");
    };
    assert_eq!(cas.candidate, original_identity);
    assert_eq!(cas.expected_active, None);
}

#[test]
fn explicit_activation_refuses_crossed_publication_and_receipt_before_io() {
    let (_batch, valid) = original_evidence();
    let mutations: [fn(&mut crate::PublishedBatchEvidence); 3] = [
        |e: &mut crate::PublishedBatchEvidence| {
            e.publication.batch_digest = "0".repeat(64);
        },
        |e: &mut crate::PublishedBatchEvidence| {
            e.receipt.manifest_digest = Some("manifest:other".into());
        },
        |e: &mut crate::PublishedBatchEvidence| {
            e.publication.target.manifest_digest = "manifest:other".into();
        },
    ];
    for mutate in mutations {
        let mut evidence = valid.clone();
        mutate(&mut evidence);
        let control = unused_control();
        let ingest = unused_ingest();
        let client = QuantaIndex::from_transports(unused_query(), control.clone(), ingest.clone());
        let error = client
            .search_corpus()
            .activate_published(&evidence, None)
            .expect_err("crossed evidence must be refused locally");
        assert!(matches!(error, crate::SdkError::Protocol(_)));
        assert!(ok_or_fail!(control.requests.lock()).is_empty());
        assert!(ok_or_fail!(ingest.requests.lock()).is_empty());
    }
}

#[test]
fn explicit_activation_refuses_zero_sequence_on_applied_receipt_before_io() {
    let (_batch, mut evidence) = original_evidence();
    evidence.receipt.applied = true;
    evidence.receipt.durable_sequence = 0;
    let control = unused_control();
    let ingest = unused_ingest();
    let client = QuantaIndex::from_transports(unused_query(), control.clone(), ingest.clone());
    let error = client
        .search_corpus()
        .activate_published(&evidence, None)
        .expect_err("an applied receipt cannot lack its durable sequence");
    assert!(matches!(error, crate::SdkError::Protocol(_)));
    assert!(ok_or_fail!(control.requests.lock()).is_empty());
    assert!(ok_or_fail!(ingest.requests.lock()).is_empty());
}

#[test]
fn publish_outcome_refuses_applied_receipt_without_durable_sequence() {
    let (batch, mut evidence) = original_evidence();
    evidence.receipt.applied = true;
    evidence.receipt.durable_sequence = 0;
    let ingest = Arc::new(StubIngestTransport::for_corpus_receipt(evidence.receipt));
    let control = unused_control();
    let client = QuantaIndex::from_transports(unused_query(), control.clone(), ingest.clone());
    let error = client
        .producer()
        .publish_search_corpus_outcome(&batch)
        .expect_err("an applied publication must identify its durable catalog sequence");
    assert!(matches!(
        error,
        crate::SdkError::Binding {
            axis: crate::ResponseBindingAxis::BatchCommitment,
            ..
        }
    ));
    assert_eq!(ok_or_fail!(ingest.requests.lock()).len(), 1);
    assert!(ok_or_fail!(control.requests.lock()).is_empty());
}

#[test]
fn observed_publish_missing_observation_retains_verified_original_evidence() {
    let (batch, evidence) = original_evidence();
    let ingest = Arc::new(StubIngestTransport::for_corpus_receipt(
        evidence.receipt.clone(),
    ));
    let control = unused_control();
    let query = unused_query();
    let client = QuantaIndex::from_transports(query.clone(), control.clone(), ingest.clone());

    let error = client
        .producer()
        .publish_search_corpus_observed(&batch)
        .expect_err("missing transient observation must retain durable publication");
    assert_eq!(
        error.published_failure_stage(),
        Some(crate::PublishedBatchFailureStage::Observation)
    );
    assert_eq!(error.published_evidence(), Some(&evidence));
    assert_eq!(error.published_publication(), Some(&evidence.publication));
    assert_eq!(error.published_receipt(), Some(&evidence.receipt));
    let cause = std::error::Error::source(&error)
        .and_then(|source| source.downcast_ref::<Box<crate::SdkError>>())
        .expect("typed boxed post-publication cause")
        .as_ref();
    assert!(matches!(
        cause,
        crate::SdkError::Protocol(message) if message.contains("observation is missing")
    ));
    assert_eq!(ok_or_fail!(ingest.requests.lock()).len(), 1);
    assert!(ok_or_fail!(control.requests.lock()).is_empty());
    assert!(ok_or_fail!(query.requests.lock()).is_empty());
}

#[test]
fn explicit_activation_rejects_wrong_root_or_token_ack_and_keeps_evidence() {
    let (_batch, evidence) = original_evidence();
    let prior = search_corpus_head(6, "manifest:prior", 1);
    let candidate = search_corpus_identity(7, "manifest:original");
    for wrong_root in [false, true] {
        let mut active = head_with_generation(candidate.clone(), if wrong_root { 2 } else { 3 });
        if wrong_root {
            active.activation_token = SearchCorpusActivationTokenV1::new(
                [8; quanta_index_contract::ACTIVATION_ROOT_INCARNATION_BYTES_V1],
                NonZeroU64::new(2).expect("positive sequence"),
            )
            .expect("nonzero incarnation");
        }
        let control = Arc::new(StubControlTransport::new(
            SearchPlaneControlIpcResponse::SearchCorpusActivationCasAck(
                SearchPlaneSearchCorpusActivationCasAck {
                    active,
                    previous_sealed_active: Some(prior.clone()),
                },
            ),
        ));
        let ingest = unused_ingest();
        let client = QuantaIndex::from_transports(unused_query(), control.clone(), ingest.clone());
        let error = client
            .search_corpus()
            .activate_published(&evidence, Some(prior.clone()))
            .expect_err("wrong root or token must not be success");
        assert_eq!(
            error.published_failure_stage(),
            Some(crate::PublishedBatchFailureStage::Activation)
        );
        assert_eq!(error.published_evidence(), Some(&evidence));
        assert_eq!(error.published_publication(), Some(&evidence.publication));
        assert_eq!(error.published_receipt(), Some(&evidence.receipt));
        let cause = std::error::Error::source(&error)
            .and_then(|source| source.downcast_ref::<Box<crate::SdkError>>())
            .expect("typed boxed post-publication cause")
            .as_ref();
        assert!(matches!(
            cause,
            crate::SdkError::Binding {
                axis: crate::ResponseBindingAxis::CasExpectation,
                ..
            }
        ));
        assert_eq!(ok_or_fail!(control.requests.lock()).len(), 1);
        assert!(ok_or_fail!(ingest.requests.lock()).is_empty());
    }
}

#[test]
fn query_only_client_reports_unavailable_planes_without_socket_calls() {
    let (batch, evidence) = original_evidence();
    let client = ok_or_fail!(QuantaIndex::connect_query_only(
        ConnectOptions::from_state_root("/tmp/qi-explicit-adversarial-query-only"),
    ));
    let activation = client
        .search_corpus()
        .activate_published(&evidence, None)
        .expect_err("query-only client has no control plane");
    assert_eq!(
        activation.published_failure_stage(),
        Some(crate::PublishedBatchFailureStage::Activation)
    );
    assert_eq!(activation.published_evidence(), Some(&evidence));
    assert_eq!(
        activation.published_publication(),
        Some(&evidence.publication)
    );
    assert_eq!(activation.published_receipt(), Some(&evidence.receipt));
    let cause = std::error::Error::source(&activation)
        .and_then(|source| source.downcast_ref::<Box<crate::SdkError>>())
        .expect("typed boxed post-publication cause")
        .as_ref();
    assert!(matches!(
        cause,
        crate::SdkError::PlaneUnavailable { plane: "control" }
    ));
    assert!(matches!(
        client.producer().publish_search_corpus_outcome(&batch),
        Err(crate::SdkError::PlaneUnavailable { plane: "ingest" })
    ));
}
