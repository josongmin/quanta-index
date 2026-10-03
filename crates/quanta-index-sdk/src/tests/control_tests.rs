use super::*;

#[test]
fn generations_rollback_emits_and_accepts_only_exact_composite_ack_v1() {
    let expected_active = search_corpus_head(11, "manifest:11", 2);
    let target = search_corpus_identity(10, "manifest:10");
    let ack = SearchPlaneSearchCorpusRollbackCasAck {
        active: head_with_generation(target.clone(), 3),
        previous_sealed_active: expected_active.clone(),
    };
    let control = Arc::new(StubControlTransport::new(
        quanta_index_contract::SearchPlaneControlIpcResponse::SearchCorpusRollbackCasAck(
            ack.clone(),
        ),
    ));
    let client = QuantaIndex::from_transports(unused_query(), control.clone(), unused_ingest());
    let observed = ok_or_fail!(client.generations().rollback(
        SearchPlaneRollbackSearchCorpusGenerationCasRequest {
            expected_active: expected_active.clone(),
            target: target.clone(),
        },
    ));
    assert_eq!(observed, ack);

    let captured = ok_or_fail!(only_control_request(control.as_ref()));
    let quanta_index_contract::SearchPlaneControlIpcRequest::RollbackSearchCorpusGenerationCas(
        request,
    ) = captured.payload
    else {
        panic!("expected composite search-corpus rollback CAS request");
    };
    assert_eq!(request.expected_active, expected_active);
    assert_eq!(request.target, target);
}

#[test]
fn generations_rollback_rejects_ack_identity_mismatches_v1() {
    let expected_active = search_corpus_head(11, "manifest:11", 2);
    let target = search_corpus_identity(10, "manifest:10");
    let cases = [
        (
            "active",
            SearchPlaneSearchCorpusRollbackCasAck {
                active: search_corpus_head(9, "manifest:9", 3),
                previous_sealed_active: expected_active.clone(),
            },
        ),
        (
            "previous",
            SearchPlaneSearchCorpusRollbackCasAck {
                active: head_with_generation(target.clone(), 3),
                previous_sealed_active: search_corpus_head(12, "manifest:12", 2),
            },
        ),
    ];
    for (label, ack) in cases {
        let control = Arc::new(StubControlTransport::new(
            quanta_index_contract::SearchPlaneControlIpcResponse::SearchCorpusRollbackCasAck(ack),
        ));
        let client = QuantaIndex::from_transports(unused_query(), control, unused_ingest());
        let error = client
            .generations()
            .rollback(SearchPlaneRollbackSearchCorpusGenerationCasRequest {
                expected_active: expected_active.clone(),
                target: target.clone(),
            })
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
            ),
            "{label} mismatch must fail as a binding error, got {error:?}"
        );
    }
}

#[test]
fn generations_rollback_rejects_invalid_composite_request_before_transport_v1() {
    let base_ack = SearchPlaneSearchCorpusRollbackCasAck {
        active: search_corpus_head(10, "manifest:10", 3),
        previous_sealed_active: search_corpus_head(11, "manifest:11", 2),
    };
    let mut malformed_target = search_corpus_identity(10, "manifest:10");
    malformed_target.semantic.track = Track::Lexical;
    let mut other_repo_target = search_corpus_identity(10, "manifest:10");
    other_repo_target.lexical.repo_id =
        RepoId::new("other-repo").expect("static fixture ID satisfies canonical policy");
    other_repo_target.semantic.repo_id =
        RepoId::new("other-repo").expect("static fixture ID satisfies canonical policy");
    let requests = [
        SearchPlaneRollbackSearchCorpusGenerationCasRequest {
            expected_active: search_corpus_head(11, "manifest:11", 2),
            target: malformed_target,
        },
        SearchPlaneRollbackSearchCorpusGenerationCasRequest {
            expected_active: search_corpus_head(11, "manifest:11", 2),
            target: other_repo_target,
        },
        SearchPlaneRollbackSearchCorpusGenerationCasRequest {
            expected_active: search_corpus_head(11, "manifest:11", 2),
            target: search_corpus_identity(11, "manifest:same-generation"),
        },
    ];

    for request in requests {
        let control = Arc::new(StubControlTransport::new(
            quanta_index_contract::SearchPlaneControlIpcResponse::SearchCorpusRollbackCasAck(
                base_ack.clone(),
            ),
        ));
        let client = QuantaIndex::from_transports(unused_query(), control.clone(), unused_ingest());
        let error = client
            .generations()
            .rollback(request)
            .expect_err("invalid composite rollback request must fail before transport");
        assert!(matches!(error, crate::SdkError::Protocol(_)));
        assert!(
            control
                .requests
                .lock()
                .expect("control request mutex")
                .is_empty(),
            "invalid composite rollback request reached the control transport"
        );
    }
}

#[test]
fn control_request_id_mismatch_is_rejected_for_activation_and_rollback_v1() {
    let candidate = search_corpus_identity(7, "manifest:request-id");
    let activation_control = Arc::new(StubControlTransport::with_request_id_offset(
        quanta_index_contract::SearchPlaneControlIpcResponse::SearchCorpusActivationCasAck(
            SearchPlaneSearchCorpusActivationCasAck {
                active: head_with_generation(candidate, 1),
                previous_sealed_active: None,
            },
        ),
        1,
    ));
    let ingest = Arc::new(StubIngestTransport::for_corpus_receipt(
        BatchPublishReceipt {
            generation: ManifestGeneration::new(7),
            manifest_digest: Some("manifest:request-id".to_string()),
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
    let client = QuantaIndex::from_transports(unused_query(), activation_control.clone(), ingest);
    let batch = SearchCorpusBatch::replace_generation(
        repo_id(),
        revision_id(),
        ManifestGeneration::new(7),
        "manifest:request-id",
    )
    .source_event(sample_source_event());
    let activation_error = client
        .producer()
        .publish_search_corpus_and_activate(&batch, None)
        .expect_err("activation response with a different request id must fail");
    assert!(
        matches!(activation_error, crate::SdkError::Protocol(ref message) if message.contains("control response request_id")),
        "activation request-id mismatch must be a protocol error, got {activation_error:?}"
    );
    assert_eq!(
        activation_control
            .requests
            .lock()
            .expect("activation control request mutex")
            .len(),
        1
    );

    let expected_active = search_corpus_head(11, "manifest:11", 2);
    let target = search_corpus_identity(10, "manifest:10");
    let rollback_control = Arc::new(StubControlTransport::with_request_id_offset(
        quanta_index_contract::SearchPlaneControlIpcResponse::SearchCorpusRollbackCasAck(
            SearchPlaneSearchCorpusRollbackCasAck {
                active: head_with_generation(target.clone(), 3),
                previous_sealed_active: expected_active.clone(),
            },
        ),
        1,
    ));
    let client =
        QuantaIndex::from_transports(unused_query(), rollback_control.clone(), unused_ingest());
    let rollback_error = client
        .generations()
        .rollback(SearchPlaneRollbackSearchCorpusGenerationCasRequest {
            expected_active,
            target,
        })
        .expect_err("rollback response with a different request id must fail");
    assert!(
        matches!(rollback_error, crate::SdkError::Protocol(ref message) if message.contains("control response request_id")),
        "rollback request-id mismatch must be a protocol error, got {rollback_error:?}"
    );
    assert_eq!(
        rollback_control
            .requests
            .lock()
            .expect("rollback control request mutex")
            .len(),
        1
    );
}

#[test]
fn generations_current_returns_snapshot_from_control_response() {
    use crate::Track;
    use quanta_index_contract::{GenerationSnapshot, SearchPlaneControlIpcResponse};
    let snapshot = GenerationSnapshot {
        repo_id: repo_id(),
        revision_id: revision_id(),
        track: Track::Lexical,
        manifest_generation: ManifestGeneration::new(11),
        manifest_digest: "digest-11".to_string(),
    };
    let control = Arc::new(StubControlTransport::new(
        SearchPlaneControlIpcResponse::CurrentGenerationSnapshot(snapshot.clone()),
    ));
    let client = QuantaIndex::from_transports(unused_query(), control.clone(), unused_ingest());
    let observed = ok_or_fail!(client.generations().current(
        repo_id(),
        revision_id(),
        Track::Lexical,
    ));
    assert_eq!(observed, snapshot);
    let captured = ok_or_fail!(
        control
            .requests
            .lock()
            .map_err(|err| crate::SdkError::Protocol(format!(
                "control requests must not be poisoned: {err}"
            )))
    )
    .first()
    .cloned();
    assert!(matches!(
        captured.map(|request| request.payload),
        Some(quanta_index_contract::SearchPlaneControlIpcRequest::CurrentGeneration(_))
    ));
}

#[test]
fn generations_current_propagates_not_ready_as_typed_remote() {
    use crate::Track;
    use quanta_index_contract::{SearchPlaneControlIpcResponse, SearchPlaneIpcError};
    let control = Arc::new(StubControlTransport::new(
        SearchPlaneControlIpcResponse::Error(SearchPlaneIpcError {
            code: SearchPlaneErrorCodeV2::NotReady,
            message: "no active Lexical generation for repo=r revision=rev".to_string(),
            repair: None,
        }),
    ));
    let client = QuantaIndex::from_transports(unused_query(), control, unused_ingest());
    let err = client
        .generations()
        .current(repo_id(), revision_id(), Track::Lexical)
        .err();
    assert!(
        matches!(err, Some(crate::SdkError::Remote { .. })),
        "expected Remote error, got {err:?}"
    );
    let Some(crate::SdkError::Remote { code, .. }) = err else {
        return;
    };
    assert_eq!(code, SearchPlaneErrorCodeV2::NotReady);
}

#[test]
fn generations_status_returns_report_with_track_records() {
    use crate::Track;
    use quanta_index_contract::{
        GenerationStatusReport, SearchPlaneControlIpcResponse, TrackReadinessRecord,
    };
    let report = GenerationStatusReport {
        repo_id: repo_id(),
        revision_id: revision_id(),
        semantic_content: None,
        tracks: vec![
            TrackReadinessRecord {
                track: Track::Lexical,
                manifest_generation: ManifestGeneration::new(7),
                manifest_digest: "digest-lex".to_string(),
            },
            TrackReadinessRecord {
                track: Track::Semantic,
                manifest_generation: ManifestGeneration::new(7),
                manifest_digest: "digest-sem".to_string(),
            },
        ],
    };
    let control = Arc::new(StubControlTransport::new(
        SearchPlaneControlIpcResponse::GenerationStatusReport(report.clone()),
    ));
    let client = QuantaIndex::from_transports(unused_query(), control, unused_ingest());
    let observed = ok_or_fail!(client.generations().status(repo_id(), revision_id()));
    assert_eq!(observed, report);
    assert_eq!(observed.tracks.len(), 2);
}

#[test]
fn generations_status_returns_empty_tracks_when_nothing_activated() {
    use quanta_index_contract::{GenerationStatusReport, SearchPlaneControlIpcResponse};
    let control = Arc::new(StubControlTransport::new(
        SearchPlaneControlIpcResponse::GenerationStatusReport(GenerationStatusReport {
            repo_id: repo_id(),
            revision_id: revision_id(),
            semantic_content: None,
            tracks: vec![],
        }),
    ));
    let client = QuantaIndex::from_transports(unused_query(), control, unused_ingest());
    let observed = ok_or_fail!(client.generations().status(repo_id(), revision_id()));
    assert!(
        observed.tracks.is_empty(),
        "QI-ACT-01: empty tracks is legitimate state, distinct from NOT_READY"
    );
}

#[test]
fn generations_active_head_binds_domain_and_does_not_map_remote_failure_to_absence() {
    use quanta_index_contract::{
        SearchCorpusActiveHeadObservationV1, SearchPlaneControlIpcRequest,
        SearchPlaneControlIpcResponse, SearchPlaneIpcError,
    };

    let head = search_corpus_head(7, "manifest:7", 3);
    let present =
        SearchCorpusActiveHeadObservationV1::new(repo_id(), revision_id(), Some(head.clone()))
            .expect("matching fixture head");
    let control = Arc::new(StubControlTransport::new(
        SearchPlaneControlIpcResponse::SearchCorpusActiveHeadObservation(present),
    ));
    let client = QuantaIndex::from_transports(unused_query(), control.clone(), unused_ingest());
    assert_eq!(
        ok_or_fail!(client.generations().active_head(repo_id(), revision_id())),
        Some(head)
    );
    assert!(matches!(
        ok_or_fail!(only_control_request(control.as_ref())).payload,
        SearchPlaneControlIpcRequest::SearchCorpusActiveHead(_)
    ));

    let absent = SearchCorpusActiveHeadObservationV1::new(repo_id(), revision_id(), None)
        .expect("explicit absence");
    let client = QuantaIndex::from_transports(
        unused_query(),
        Arc::new(StubControlTransport::new(
            SearchPlaneControlIpcResponse::SearchCorpusActiveHeadObservation(absent),
        )),
        unused_ingest(),
    );
    assert_eq!(
        ok_or_fail!(client.generations().active_head(repo_id(), revision_id())),
        None
    );

    let foreign = SearchCorpusActiveHeadObservationV1::new(
        RepoId::new("foreign").expect("canonical fixture"),
        revision_id(),
        None,
    )
    .expect("explicit foreign absence");
    let client = QuantaIndex::from_transports(
        unused_query(),
        Arc::new(StubControlTransport::new(
            SearchPlaneControlIpcResponse::SearchCorpusActiveHeadObservation(foreign),
        )),
        unused_ingest(),
    );
    assert!(matches!(
        client.generations().active_head(repo_id(), revision_id()),
        Err(crate::SdkError::Binding {
            axis: crate::ResponseBindingAxis::TargetIdentity,
            ..
        })
    ));

    let client = QuantaIndex::from_transports(
        unused_query(),
        Arc::new(StubControlTransport::new(
            SearchPlaneControlIpcResponse::Error(SearchPlaneIpcError {
                code: SearchPlaneErrorCodeV2::NotReady,
                message: "catalog durability uncertain".to_string(),
                repair: None,
            }),
        )),
        unused_ingest(),
    );
    assert!(matches!(
        client.generations().active_head(repo_id(), revision_id()),
        Err(crate::SdkError::Remote {
            code: SearchPlaneErrorCodeV2::NotReady,
            ..
        })
    ));
}

/// QI-BB-015: the metrics scrape rides the control socket and comes back
/// as the typed snapshot, exactly as the daemon encoded it.
#[test]
fn observability_metrics_snapshot_returns_the_daemon_snapshot() {
    use quanta_index_contract::{
        MetricBucketV1, MetricCounterV1, MetricGaugeV1, MetricHistogramV1, MetricsDiagnosticsV1,
        MetricsSnapshotV1, SearchPlaneControlIpcRequest, SearchPlaneControlIpcResponse,
    };
    let snapshot = MetricsSnapshotV1 {
        counters: vec![MetricCounterV1 {
            name: "lq_query_intake_total".to_string(),
            value: 12,
        }],
        gauges: vec![MetricGaugeV1 {
            name: "ipc_query_connections_live".to_string(),
            value: 1.0,
        }],
        histograms: vec![MetricHistogramV1 {
            name: "lq_route_lexical_latency_ms".to_string(),
            count: 2,
            sum: 7.0,
            min: 3.0,
            max: 4.0,
            buckets: vec![MetricBucketV1 { le: 5.0, count: 2 }],
        }],
        diagnostics: MetricsDiagnosticsV1 {
            samples_recorded: 14,
            samples_dropped: 0,
            errors_recorded: 0,
            errors_dropped: 0,
        },
    };
    let control = Arc::new(StubControlTransport::new(
        SearchPlaneControlIpcResponse::MetricsSnapshot(snapshot.clone()),
    ));
    let client = QuantaIndex::from_transports(unused_query(), control.clone(), unused_ingest());
    let observed = ok_or_fail!(client.observability().metrics_snapshot());
    assert_eq!(observed, snapshot);
    let sent: Vec<SearchPlaneControlIpcRequestEnvelope> = ok_or_fail!(
        control
            .requests
            .lock()
            .map(|requests| requests.clone())
            .map_err(|err| crate::SdkError::Protocol(err.to_string()))
    );
    assert_eq!(sent.len(), 1);
    assert!(
        matches!(
            sent.first().map(|request| &request.payload),
            Some(SearchPlaneControlIpcRequest::MetricsSnapshot(_))
        ),
        "the scrape request is what went over the wire: {sent:?}"
    );
    // The control client is the same call under its own name.
    let control = Arc::new(StubControlTransport::new(
        SearchPlaneControlIpcResponse::MetricsSnapshot(snapshot.clone()),
    ));
    let client = QuantaIndex::from_transports(unused_query(), control, unused_ingest());
    let via_control = ok_or_fail!(client.control().metrics_snapshot());
    assert_eq!(via_control, snapshot);
}

/// QI-BB-015: a control answer of the wrong kind is a protocol error, and a
/// typed daemon refusal surfaces as the remote error it is.
#[test]
fn observability_metrics_snapshot_refuses_wrong_kind_and_surfaces_remote_errors() {
    use quanta_index_contract::{
        GenerationStatusReport, SearchPlaneControlIpcResponse, SearchPlaneIpcError,
    };
    let wrong_kind = Arc::new(StubControlTransport::new(
        SearchPlaneControlIpcResponse::GenerationStatusReport(GenerationStatusReport {
            repo_id: repo_id(),
            revision_id: revision_id(),
            semantic_content: None,
            tracks: vec![],
        }),
    ));
    let client = QuantaIndex::from_transports(unused_query(), wrong_kind, unused_ingest());
    match client.observability().metrics_snapshot() {
        Err(crate::SdkError::Binding { actual, .. }) => {
            assert_eq!(
                actual, "generation_status_report",
                "the binding error names what arrived"
            );
        }
        other => panic!("expected a binding error, got {other:?}"),
    }
    let refused = Arc::new(StubControlTransport::new(
        SearchPlaneControlIpcResponse::Error(SearchPlaneIpcError {
            code: SearchPlaneErrorCodeV2::MetricsSourceDefect,
            message: "metrics scrape: source point name `Bad` is not [a-z][a-z0-9_]*".to_string(),
            repair: None,
        }),
    ));
    let client = QuantaIndex::from_transports(unused_query(), refused, unused_ingest());
    match client.observability().metrics_snapshot() {
        Err(crate::SdkError::Remote { code, message, .. }) => {
            assert_eq!(code, SearchPlaneErrorCodeV2::MetricsSourceDefect);
            assert!(message.contains("`Bad`"), "{message}");
        }
        other => panic!("expected the daemon's typed refusal, got {other:?}"),
    }
}

#[test]
fn process_readiness_binds_the_control_route_and_rejects_forged_green() {
    use quanta_index_contract::{
        ProcessComponentsHealthV1, ProcessProviderClaimV1, ProcessProviderReadinessV1,
        ProcessReadinessPhaseV1, ProcessReadinessV1, SearchPlaneControlIpcRequest,
        SearchPlaneControlIpcResponse,
    };
    let report = ProcessReadinessV1 {
        ready: true,
        supervisor_phase: ProcessReadinessPhaseV1::Ready,
        components: ProcessComponentsHealthV1 {
            query_plane: true,
            control_plane: true,
            ingest_plane: true,
            maintenance_heartbeat: true,
            required_backend: true,
            provider: ProcessProviderReadinessV1 {
                claim: ProcessProviderClaimV1::Degraded,
                healthy: false,
            },
        },
        active_candidate_integrity: None,
        active_repositories: 0,
        not_ready_reasons: Vec::new(),
    };
    let control = Arc::new(StubControlTransport::new(
        SearchPlaneControlIpcResponse::ProcessReadinessReport(report.clone()),
    ));
    let client = QuantaIndex::from_transports(unused_query(), control.clone(), unused_ingest());
    assert_eq!(
        ok_or_fail!(client.observability().process_readiness()),
        report
    );
    let sent_readiness = {
        let sent = control.requests.lock().expect("stub request lock");
        matches!(
            sent.first().map(|request| &request.payload),
            Some(SearchPlaneControlIpcRequest::ProcessReadiness(_))
        )
    };
    assert!(sent_readiness);

    let mut forged = report;
    forged.components.query_plane = false;
    let control = Arc::new(StubControlTransport::new(
        SearchPlaneControlIpcResponse::ProcessReadinessReport(forged),
    ));
    let client = QuantaIndex::from_transports(unused_query(), control, unused_ingest());
    assert!(matches!(
        client.observability().process_readiness(),
        Err(crate::SdkError::Protocol(_))
    ));
}

#[test]
fn request_events_bind_plane_and_limit_before_sdk_publication() {
    use quanta_index_contract::{
        ProcessRequestEventPlaneV1, ProcessRequestEventsV1, SearchPlaneControlIpcResponse,
    };

    let response = ProcessRequestEventsV1 {
        process_instance: "0000000000000000000000000000002a".to_owned(),
        plane: ProcessRequestEventPlaneV1::Query,
        events: Vec::new(),
        oldest_retained_sequence: None,
        next_sequence: 1,
        dropped_before: 0,
        dropped_after: 0,
        omitted_before_window: false,
        sequence_exhausted: false,
    };
    let control = Arc::new(StubControlTransport::new(
        SearchPlaneControlIpcResponse::ProcessRequestEventsV1(response.clone()),
    ));
    let client = QuantaIndex::from_transports(unused_query(), control, unused_ingest());
    assert_eq!(
        ok_or_fail!(
            client
                .observability()
                .request_events(ProcessRequestEventPlaneV1::Query, 1)
        ),
        response
    );

    let control = Arc::new(StubControlTransport::new(
        SearchPlaneControlIpcResponse::ProcessRequestEventsV1(response),
    ));
    let client = QuantaIndex::from_transports(unused_query(), control, unused_ingest());
    assert!(matches!(
        client
            .observability()
            .request_events(ProcessRequestEventPlaneV1::Control, 1),
        Err(crate::SdkError::Binding { .. })
    ));
    assert!(matches!(
        client
            .observability()
            .request_events(ProcessRequestEventPlaneV1::Query, 0),
        Err(crate::SdkError::Protocol(_))
    ));
}

/// QI-BB-026: the quarantine listing rides the control socket and comes
/// back as the typed inventory; a discard sends the target verbatim and
/// returns the daemon's ack once it names the same target.
#[test]
fn quarantine_inventory_and_discard_carry_the_target_verbatim() {
    use quanta_index_contract::{
        QuarantineDiscardAck, QuarantineDiscardOutcomeDtoV1, QuarantineInventoryV1,
        QuarantineTargetV1, QuarantinedGenerationEntryV1, QuarantinedRepoMapFileEntryV1,
        SearchPlaneControlIpcRequest, SearchPlaneControlIpcResponse, SearchPlaneTrackKind,
    };
    let inventory = QuarantineInventoryV1 {
        lexical: vec![QuarantinedGenerationEntryV1 {
            track: SearchPlaneTrackKind::Lexical,
            path: "/state/indexes/lexical/repo/rev/g2".to_string(),
            reason: "GENERATION_QUARANTINE_IDENTITY_UNREADABLE".to_string(),
            detail: "identity file does not decode".to_string(),
        }],
        semantic: Vec::new(),
        repo_map: vec![QuarantinedRepoMapFileEntryV1 {
            file_name: "stale--marker.json".to_string(),
            reason: "snapshot does not decode".to_string(),
        }],
    };
    let control = Arc::new(StubControlTransport::new(
        SearchPlaneControlIpcResponse::QuarantineInventory(inventory.clone()),
    ));
    let client = QuantaIndex::from_transports(unused_query(), control.clone(), unused_ingest());
    let listed = ok_or_fail!(client.quarantine().inventory());
    assert_eq!(listed, inventory);
    let sent = ok_or_fail!(only_control_request(&control));
    assert!(
        matches!(
            sent.payload,
            SearchPlaneControlIpcRequest::QuarantineInventory(_)
        ),
        "the inventory request is what went over the wire: {sent:?}"
    );

    let target = QuarantineTargetV1::RepoMapFile(QuarantinedRepoMapFileEntryV1 {
        file_name: "stale--marker.json".to_string(),
        reason: "snapshot does not decode".to_string(),
    });
    let ack = QuarantineDiscardAck {
        target: target.clone(),
        outcome: QuarantineDiscardOutcomeDtoV1::Discarded { bytes: 15 },
    };
    let control = Arc::new(StubControlTransport::new(
        SearchPlaneControlIpcResponse::QuarantineDiscardAck(ack.clone()),
    ));
    let client = QuantaIndex::from_transports(unused_query(), control.clone(), unused_ingest());
    let observed = ok_or_fail!(client.quarantine().discard(&target));
    assert_eq!(observed, ack);
    let sent = ok_or_fail!(only_control_request(&control));
    let SearchPlaneControlIpcRequest::QuarantineDiscard(request) = sent.payload else {
        panic!("expected the discard request on the wire, got {sent:?}");
    };
    assert_eq!(
        request.target, target,
        "the target goes over the wire verbatim"
    );
}

/// QI-BB-026: a mismatched ack, a wrong kind and a typed refusal each
/// surface as what they are.
///
/// An ack that names a different target than was sent is a protocol
/// error (the daemon answered some other discard); an answer of the wrong
/// kind is a protocol error; the daemon's typed refusal of a stale target
/// surfaces as the remote error it is.
#[test]
fn quarantine_discard_refuses_mismatched_acks_wrong_kinds_and_surfaces_refusals() {
    use quanta_index_contract::{
        QuarantineDiscardAck, QuarantineDiscardOutcomeDtoV1, QuarantineInventoryV1,
        QuarantineTargetV1, QuarantinedGenerationEntryV1, SearchPlaneControlIpcResponse,
        SearchPlaneIpcError, SearchPlaneTrackKind,
    };
    let entry = |path: &str| QuarantinedGenerationEntryV1 {
        track: SearchPlaneTrackKind::Semantic,
        path: path.to_string(),
        reason: "GENERATION_QUARANTINE_SCOPE_MISMATCH".to_string(),
        detail: String::new(),
    };
    let sent = QuarantineTargetV1::Generation(entry("/state/indexes/semantic/repo/rev/g5"));
    let mismatched = Arc::new(StubControlTransport::new(
        SearchPlaneControlIpcResponse::QuarantineDiscardAck(QuarantineDiscardAck {
            target: QuarantineTargetV1::Generation(entry("/state/indexes/semantic/repo/rev/g6")),
            outcome: QuarantineDiscardOutcomeDtoV1::Absent,
        }),
    ));
    let client = QuantaIndex::from_transports(unused_query(), mismatched, unused_ingest());
    match client.quarantine().discard(&sent) {
        Err(crate::SdkError::Binding { axis, .. }) => {
            assert_eq!(
                axis,
                crate::ResponseBindingAxis::TargetIdentity,
                "the binding error says the ack is for another target"
            );
        }
        other => panic!("expected a binding error, got {other:?}"),
    }
    let wrong_kind = Arc::new(StubControlTransport::new(
        SearchPlaneControlIpcResponse::QuarantineInventory(QuarantineInventoryV1::default()),
    ));
    let client = QuantaIndex::from_transports(unused_query(), wrong_kind, unused_ingest());
    match client.quarantine().discard(&sent) {
        Err(crate::SdkError::Binding { actual, .. }) => {
            assert_eq!(
                actual, "quarantine_inventory",
                "the binding error names what arrived"
            );
        }
        other => panic!("expected a binding error, got {other:?}"),
    }
    let wrong_kind = Arc::new(StubControlTransport::new(
        SearchPlaneControlIpcResponse::QuarantineDiscardAck(QuarantineDiscardAck {
            target: sent.clone(),
            outcome: QuarantineDiscardOutcomeDtoV1::Absent,
        }),
    ));
    let client = QuantaIndex::from_transports(unused_query(), wrong_kind, unused_ingest());
    match client.quarantine().inventory() {
        Err(crate::SdkError::Binding { actual, .. }) => {
            assert_eq!(
                actual, "quarantine_discard_ack",
                "the binding error names what arrived"
            );
        }
        other => panic!("expected a binding error, got {other:?}"),
    }
    let refused = Arc::new(StubControlTransport::new(
        SearchPlaneControlIpcResponse::Error(SearchPlaneIpcError {
            code: SearchPlaneErrorCodeV2::QuarantineTargetNotQuarantined,
            message: "semantic: refusing to discard /state/indexes/semantic/repo/rev/g5: it is quarantined as GENERATION_QUARANTINE_IDENTITY_UNREADABLE now".to_string(),
            repair: None,
        }),
    ));
    let client = QuantaIndex::from_transports(unused_query(), refused, unused_ingest());
    match client.quarantine().discard(&sent) {
        Err(crate::SdkError::Remote { code, message, .. }) => {
            assert_eq!(code, SearchPlaneErrorCodeV2::QuarantineTargetNotQuarantined);
            assert!(
                message.contains("list again") || message.contains("now"),
                "{message}"
            );
        }
        other => panic!("expected the daemon's typed refusal, got {other:?}"),
    }
}

#[test]
fn search_corpus_public_surface_keeps_legacy_lexical_ingest_names_out_v1() {
    let sdk_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let lexical_source =
        std::fs::read_to_string(sdk_root.join("src/lexical.rs")).expect("read lexical.rs");
    let client_source =
        std::fs::read_to_string(sdk_root.join("src/client.rs")).expect("read client.rs");
    let public_surface = std::fs::read_to_string(sdk_root.join("src/lib.rs")).expect("read lib.rs");
    let contract_ingest =
        std::fs::read_to_string(sdk_root.join("../quanta-index-contract/src/ipc/ingest.rs"))
            .expect("read contract ingest.rs");

    for forbidden in [
        "LexicalIngestBatch",
        "LexicalReplaceScope",
        "LexicalTombstoneScope",
        "PublishLexicalBatch",
        "publish_lexical",
        "DirectLexicalMaterializer",
        "LexicalIngestPort",
    ] {
        assert!(
            !lexical_source.contains(forbidden)
                && !client_source.contains(forbidden)
                && !public_surface.contains(forbidden)
                && !contract_ingest.contains(forbidden),
            "legacy lexical-ingest symbol must stay deleted from public ingest surfaces: {forbidden}",
        );
    }

    for required in [
        "SearchCorpusBatch",
        "publish_search_corpus",
        "publish_search_corpus_and_activate",
        "PublishSearchCorpusBatch",
        "SearchCorpusIngestBatch",
    ] {
        assert!(
            lexical_source.contains(required)
                || client_source.contains(required)
                || public_surface.contains(required)
                || contract_ingest.contains(required),
            "search-corpus ingest owner surface must keep `{required}` wired",
        );
    }
}

// W7 (plan §11): the pre-contract auxiliary channel ops were deleted from the
// contract because no producer, adapter or ledger path constructs them; the
// typed ingest batches are the only aux ingress. Their names must not come
// back on the contract surface.
#[test]
fn contract_channel_surface_keeps_the_deleted_auxiliary_op_names_out_v1() {
    let sdk_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let contract_root = sdk_root.join("../quanta-index-contract/src");
    let surfaces = [
        std::fs::read_to_string(contract_root.join("channel/ops.rs")).expect("read channel/ops.rs"),
        std::fs::read_to_string(contract_root.join("channel/mod.rs")).expect("read channel/mod.rs"),
        std::fs::read_to_string(contract_root.join("lib.rs")).expect("read contract lib.rs"),
        std::fs::read_to_string(sdk_root.join("src/lib.rs")).expect("read sdk lib.rs"),
    ];
    for forbidden in [
        "DeleteChunk",
        "DeleteSymbol",
        "UpsertTag",
        "DeleteRef",
        "DeleteTag",
        "UpsertDirty",
        "EvictDirty",
        "DeleteParseTree",
        "TombstoneStructuralScope",
        "UpsertDiffHunk",
    ] {
        assert!(
            surfaces.iter().all(|source| !source.contains(forbidden)),
            "deleted auxiliary channel op must stay off the contract surface: {forbidden}",
        );
    }
}
