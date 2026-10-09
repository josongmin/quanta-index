use super::*;

struct L2ReplayIngestTransport {
    outcome: quanta_index_contract::SearchCorpusPublishOutcome,
    observed: bool,
}

impl IngestTransport for L2ReplayIngestTransport {
    fn send(
        &self,
        request: SearchPlaneIngestIpcRequestEnvelope,
    ) -> Result<SearchPlaneIngestIpcResponseEnvelope, crate::SdkError> {
        let SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(batch) = request.payload else {
            return Err(crate::SdkError::Protocol("expected corpus replay".into()));
        };
        let mut outcome = self.outcome.clone();
        if self.observed {
            outcome.observation = Some(quanta_index_contract::SearchCorpusIngestObservation {
                request_id: request.request_id,
                repo_id: batch.repo_id,
                revision_id: batch.revision_id,
                generation: batch.generation,
                batch_digest: batch.batch_digest,
                status: quanta_index_contract::IngestObservationStatus::Replayed,
                semantic: None,
                lexical_build_ns: None,
                lexical_stages: None,
                finalize_ns: None,
                activation_ns: None,
            });
        }
        Ok(SearchPlaneIngestIpcResponseEnvelope {
            request_id: request.request_id,
            payload: SearchPlaneIngestIpcResponse::SearchCorpusReceipt(outcome),
        })
    }
}

fn l2_replay_fixture() -> (
    SearchCorpusBatch,
    quanta_index_contract::SearchCorpusPublishOutcome,
) {
    let original = SearchCorpusBatch::replace_generation(
        repo_id(),
        revision_id(),
        ManifestGeneration::new(7),
        "manifest:original",
    )
    .source_event(sample_source_event());
    let wire = ok_or_fail!(original.to_wire_batch());
    let outcome = quanta_index_contract::SearchCorpusPublishOutcome {
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
        observation: None,
    };
    let replay = SearchCorpusBatch::replace_generation(
        repo_id(),
        ok_or_fail!(RevisionId::new("retargeted-revision")),
        ManifestGeneration::new(99),
        "manifest:retargeted",
    )
    .source_event(sample_source_event());
    assert_ne!(
        ok_or_fail!(replay.batch_digest()),
        outcome.receipt.batch_digest
    );
    (replay, outcome)
}

#[test]
fn l2_source_replay_publishes_original_receipt_and_activates_original_pair() {
    let (batch, expected) = l2_replay_fixture();
    let original = search_corpus_identity(7, "manifest:original");
    for observed in [false, true] {
        let ack = SearchPlaneSearchCorpusActivationCasAck {
            active: head_with_generation(original.clone(), 1),
            previous_sealed_active: None,
        };
        let control = Arc::new(StubControlTransport::new(
            quanta_index_contract::SearchPlaneControlIpcResponse::SearchCorpusActivationCasAck(
                ack.clone(),
            ),
        ));
        let client = QuantaIndex::from_transports(
            unused_query(),
            control.clone(),
            Arc::new(L2ReplayIngestTransport {
                outcome: expected.clone(),
                observed,
            }),
        );
        if observed {
            let (outcome, actual_ack, _sdk_timings) = ok_or_fail!(
                client
                    .search_corpus()
                    .publish_and_activate_observed(&batch, None)
            );
            assert_eq!(outcome.publication, expected.publication);
            assert_eq!(outcome.receipt, expected.receipt);
            assert_eq!(actual_ack, ack);
            assert_eq!(
                outcome.observation.expect("observed replay").generation,
                batch.generation()
            );
        } else {
            let (receipt, actual_ack) =
                ok_or_fail!(client.search_corpus().publish_and_activate(&batch, None));
            assert_eq!(receipt, expected.receipt);
            assert_eq!(actual_ack, ack);
        }
        let request = ok_or_fail!(only_control_request(&control));
        let quanta_index_contract::SearchPlaneControlIpcRequest::ActivateSearchCorpusGenerationCas(
            request,
        ) = request.payload
        else {
            panic!("expected paired activation");
        };
        assert_eq!(request.candidate, original);
        assert_eq!(request.expected_active, None);
    }
    for observed in [false, true] {
        let client = QuantaIndex::from_transports(
            unused_query(),
            unused_control(),
            Arc::new(L2ReplayIngestTransport {
                outcome: expected.clone(),
                observed,
            }),
        );
        let receipt = if observed {
            ok_or_fail!(client.producer().publish_search_corpus_observed(&batch)).receipt
        } else {
            ok_or_fail!(client.search_corpus().publish(&batch))
        };
        assert_eq!(receipt, expected.receipt);
    }
}

#[test]
fn l2_source_replay_retains_verified_receipt_when_activation_fails() {
    let (batch, published) = l2_replay_fixture();
    let control = unused_control();
    let client = QuantaIndex::from_transports(
        unused_query(),
        control.clone(),
        Arc::new(L2ReplayIngestTransport {
            outcome: published.clone(),
            observed: false,
        }),
    );
    let error = client
        .search_corpus()
        .publish_and_activate(&batch, None)
        .expect_err("activation refusal must retain publication evidence");
    assert_eq!(error.published_receipt(), Some(&published.receipt));
    assert_eq!(error.published_publication(), Some(&published.publication));
    assert_eq!(
        error
            .published_publication()
            .expect("original publication")
            .target
            .manifest_generation,
        ManifestGeneration::new(7)
    );
    assert!(matches!(
        error,
        crate::SdkError::AfterPublish { source, .. }
            if matches!(*source, crate::SdkError::Remote { code: SearchPlaneErrorCodeV2::Internal, .. })
    ));
    assert_eq!(ok_or_fail!(control.requests.lock()).len(), 1);
}

#[test]
fn l2_source_replay_rejects_false_bindings_with_or_without_observation() {
    let (batch, valid) = l2_replay_fixture();
    let mutations: [fn(&mut quanta_index_contract::SearchCorpusPublishOutcome); 14] = [
        |o| o.publication.event.stream_id = "other".into(),
        |o| o.publication.event.event_id = "other".into(),
        |o| o.publication.event.expected_base_event_id = Some("other".into()),
        |o| o.publication.event.payload_sha256 = [1; 32],
        |o| o.publication.target.repo_id = RepoId::new("other").expect("fixture repo"),
        |o| o.publication.target.track = quanta_index_contract::SearchPlaneTrackKind::Semantic,
        |o| o.publication.target.manifest_generation = ManifestGeneration::new(8),
        |o| o.publication.target.manifest_digest = "other".into(),
        |o| o.publication.batch_digest = "a".repeat(64),
        |o| o.receipt.batch_digest = "b".repeat(64),
        |o| o.receipt.generation = ManifestGeneration::new(99),
        |o| o.receipt.applied = true,
        |o| o.receipt.sealed = false,
        |o| o.receipt.durable_sequence = 0,
    ];
    for mutate in mutations {
        for observed in [false, true] {
            let mut outcome = valid.clone();
            mutate(&mut outcome);
            let control = unused_control();
            let client = QuantaIndex::from_transports(
                unused_query(),
                control.clone(),
                Arc::new(L2ReplayIngestTransport { outcome, observed }),
            );
            assert!(
                client
                    .search_corpus()
                    .publish_and_activate(&batch, None)
                    .is_err()
            );
            assert!(ok_or_fail!(control.requests.lock()).is_empty());
        }
    }
}

#[test]
fn l2_source_replay_does_not_redirect_explicit_cas_expectation() {
    let (batch, outcome) = l2_replay_fixture();
    let mut expected = search_corpus_head(50, "manifest:current", 3);
    expected.generation.lexical.revision_id = batch.revision_id().clone();
    expected.generation.semantic.revision_id = batch.revision_id().clone();
    let control = unused_control();
    let client = QuantaIndex::from_transports(
        unused_query(),
        control.clone(),
        Arc::new(L2ReplayIngestTransport {
            outcome: outcome.clone(),
            observed: true,
        }),
    );
    let error = client
        .search_corpus()
        .publish_and_activate_observed(&batch, Some(expected))
        .expect_err("original target cannot advance the requested revision's head");
    assert_eq!(error.published_receipt(), Some(&outcome.receipt));
    assert_eq!(error.published_publication(), Some(&outcome.publication));
    assert!(matches!(error, crate::SdkError::AfterPublish { source, .. }
            if matches!(*source, crate::SdkError::Protocol(ref message) if message.contains("composite activation request is invalid"))));
    assert!(ok_or_fail!(control.requests.lock()).is_empty());
}

#[test]
fn l2_source_replay_validates_cas_against_original_target_with_explicit_head() {
    let (_, outcome) = l2_replay_fixture();
    // The submitted G3 cannot advance G6; the verified original G7 can.
    let batch = SearchCorpusBatch::replace_generation(
        repo_id(),
        ok_or_fail!(RevisionId::new("retargeted-revision")),
        ManifestGeneration::new(3),
        "manifest:retargeted",
    )
    .source_event(sample_source_event());
    let previous = search_corpus_head(6, "manifest:previous", 1);
    let original = search_corpus_identity(7, "manifest:original");
    for observed in [false, true] {
        let ack = SearchPlaneSearchCorpusActivationCasAck {
            active: head_with_generation(original.clone(), 2),
            previous_sealed_active: Some(previous.clone()),
        };
        let control = Arc::new(StubControlTransport::new(
            quanta_index_contract::SearchPlaneControlIpcResponse::SearchCorpusActivationCasAck(
                ack.clone(),
            ),
        ));
        let client = QuantaIndex::from_transports(
            unused_query(),
            control.clone(),
            Arc::new(L2ReplayIngestTransport {
                outcome: outcome.clone(),
                observed,
            }),
        );
        if observed {
            let (published, active, _) = ok_or_fail!(
                client
                    .search_corpus()
                    .publish_and_activate_observed(&batch, Some(previous.clone()))
            );
            assert_eq!(published.publication, outcome.publication);
            assert_eq!(published.receipt, outcome.receipt);
            assert_eq!(active, ack);
        } else {
            let (receipt, active) = ok_or_fail!(
                client
                    .search_corpus()
                    .publish_and_activate(&batch, Some(previous.clone()))
            );
            assert_eq!(receipt, outcome.receipt);
            assert_eq!(active, ack);
        }
        let request = ok_or_fail!(only_control_request(&control));
        let quanta_index_contract::SearchPlaneControlIpcRequest::ActivateSearchCorpusGenerationCas(
            request,
        ) = request.payload
        else {
            panic!("expected composite activation");
        };
        assert_eq!(request.candidate, original);
        assert_eq!(request.expected_active, Some(previous.clone()));
    }
}
