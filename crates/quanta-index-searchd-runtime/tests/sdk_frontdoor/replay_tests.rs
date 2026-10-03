use super::*;

// L2: real SDK + sockets + durable journal/catalog + paired storage. The
// original source publication survives retargeting and a runtime restart.
#[test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "test assertions check the oracle after fallible setup"
)]
fn l2_source_replay_keeps_original_publication_through_sdk_activation_and_restart() -> TestResult {
    use quanta_index_contract::{IngestObservationStatus, SourcePublicationEvent};
    let root = quanta_index_searchd_harness::private_tempdir()?;
    let fixture = SdkFrontdoorRuntime::start_at(root.path())?;
    let event = SourcePublicationEvent {
        stream_id: "l2-sdk-stream".into(),
        event_id: "l2-sdk-event".into(),
        expected_base_event_id: None,
        payload_sha256: [0; 32], // The SDK commits the actual empty payload.
    };
    let original = SearchCorpusBatch::replace_generation(
        repo(),
        revision(),
        ManifestGeneration::new(1),
        "manifest:l2-original",
    )
    .source_event(event.clone());
    let first = fixture
        .client
        .producer()
        .publish_search_corpus_observed(&original)?;
    assert!(first.receipt.applied);
    assert!(first.receipt.durable_sequence > 0);
    let requested_revision = RevisionId::new("l2-retargeted-revision")?;
    let retargeted = SearchCorpusBatch::replace_generation(
        repo(),
        requested_revision.clone(),
        ManifestGeneration::new(99),
        "manifest:l2-retargeted",
    )
    .source_event(event);
    assert_ne!(retargeted.batch_digest()?, first.receipt.batch_digest);
    let (replayed, activation, _sdk_timings) = fixture
        .client
        .search_corpus()
        .publish_and_activate_observed(&retargeted, None)?;
    assert_eq!(replayed.publication, first.publication);
    assert_eq!(replayed.receipt, first.receipt.clone().replayed());
    let observation = replayed
        .observation
        .as_ref()
        .ok_or("missing replay observation")?;
    assert_eq!(observation.status, IngestObservationStatus::Replayed);
    assert_eq!(observation.revision_id, requested_revision);
    assert_eq!(observation.generation, ManifestGeneration::new(99));
    assert!(observation.semantic.is_none());
    assert!(observation.lexical_build_ns.is_none());
    assert_eq!(
        activation.active.generation.lexical,
        first.publication.target
    );
    assert_eq!(
        activation.active.generation.semantic.manifest_generation,
        ManifestGeneration::new(1)
    );
    assert_eq!(
        current_sdk_search_corpus_or_none(&fixture.client, repo(), revision())?,
        Some(activation.active.clone())
    );
    assert_eq!(
        current_sdk_search_corpus_or_none(&fixture.client, repo(), requested_revision.clone())?,
        None
    );
    fixture.stop()?;

    let restarted = SdkFrontdoorRuntime::start_at(root.path())?;
    let replayed = restarted
        .client
        .producer()
        .publish_search_corpus_observed(&retargeted)?;
    assert_eq!(replayed.publication, first.publication);
    assert_eq!(replayed.receipt, first.receipt.replayed());
    assert_eq!(
        current_sdk_search_corpus_or_none(&restarted.client, repo(), revision())?,
        Some(activation.active)
    );
    assert_eq!(
        current_sdk_search_corpus_or_none(&restarted.client, repo(), requested_revision)?,
        None
    );
    restarted.stop()
}
