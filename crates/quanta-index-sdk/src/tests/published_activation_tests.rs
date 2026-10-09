use super::*;

fn original_publication() -> (SearchCorpusBatch, crate::PublishedBatchEvidence) {
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
fn explicit_activation_uses_original_evidence_and_never_republishes() {
    let (_batch, evidence) = original_publication();
    let prior = search_corpus_head(6, "manifest:prior", 2);
    let ack = SearchPlaneSearchCorpusActivationCasAck {
        active: head_with_generation(search_corpus_identity(7, "manifest:original"), 3),
        previous_sealed_active: Some(prior.clone()),
    };
    let control = Arc::new(StubControlTransport::new(
        quanta_index_contract::SearchPlaneControlIpcResponse::SearchCorpusActivationCasAck(
            ack.clone(),
        ),
    ));
    let ingest = unused_ingest();
    let client = QuantaIndex::from_transports(unused_query(), control.clone(), ingest.clone());
    assert_eq!(
        ok_or_fail!(
            client
                .producer()
                .activate_published_search_corpus(&evidence, Some(prior.clone()))
        ),
        ack
    );
    assert!(ok_or_fail!(ingest.requests.lock()).is_empty());
    let request = ok_or_fail!(only_control_request(&control));
    let quanta_index_contract::SearchPlaneControlIpcRequest::ActivateSearchCorpusGenerationCas(
        request,
    ) = request.payload
    else {
        panic!("expected activation CAS");
    };
    assert_eq!(
        request.candidate,
        search_corpus_identity(7, "manifest:original")
    );
    assert_eq!(request.expected_active, Some(prior));
}

#[test]
fn explicit_activation_refusal_retains_original_evidence_without_ingest() {
    let (_batch, evidence) = original_publication();
    let control = unused_control();
    let ingest = unused_ingest();
    let client = QuantaIndex::from_transports(unused_query(), control.clone(), ingest.clone());
    let error = client
        .search_corpus()
        .activate_published(&evidence, None)
        .expect_err("remote refusal");
    assert_eq!(error.published_evidence(), Some(&evidence));
    assert_eq!(error.published_publication(), Some(&evidence.publication));
    assert_eq!(error.published_receipt(), Some(&evidence.receipt));
    assert!(ok_or_fail!(ingest.requests.lock()).is_empty());
    assert_eq!(ok_or_fail!(control.requests.lock()).len(), 1);
}

#[test]
fn explicit_activation_rejects_malformed_evidence_before_io() {
    let (_batch, valid) = original_publication();
    let mutations: [fn(&mut crate::PublishedBatchEvidence); 6] = [
        |e| e.receipt.sealed = false,
        |e| e.receipt.generation = ManifestGeneration::new(99),
        |e| e.receipt.batch_digest = "b".repeat(64),
        |e| e.receipt.durable_sequence = 0,
        |e| e.receipt.semantic_content = None,
        |e| e.publication.target.track = Track::Semantic,
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
            .expect_err("malformed evidence");
        assert!(matches!(error, crate::SdkError::Protocol(_)));
        assert!(ok_or_fail!(control.requests.lock()).is_empty());
        assert!(ok_or_fail!(ingest.requests.lock()).is_empty());
    }
}

#[test]
fn publish_outcome_does_not_require_transient_observation() {
    let (batch, evidence) = original_publication();
    let ingest = Arc::new(StubIngestTransport::for_corpus_receipt(
        evidence.receipt.clone(),
    ));
    let client = QuantaIndex::from_transports(unused_query(), unused_control(), ingest.clone());
    let outcome = ok_or_fail!(client.producer().publish_search_corpus_outcome(&batch));
    assert!(outcome.observation.is_none());
    assert_eq!(crate::PublishedBatchEvidence::from(&outcome), evidence);
    assert_eq!(ok_or_fail!(ingest.requests.lock()).len(), 1);
}
