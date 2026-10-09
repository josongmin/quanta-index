use super::*;

#[test]
fn parses_generation_status_request() {
    let parsed = ParsedCommand::parse([
        "generation-status",
        "--repo-id",
        "repo-1",
        "--revision-id",
        "rev-1",
    ]);
    assert!(parsed.is_ok());
    let Ok(parsed) = parsed else {
        return;
    };
    assert_eq!(parsed.kind, CommandKind::GenerationStatus);
    let CliRequest::GenerationStatus {
        repo_id,
        revision_id,
    } = parsed.request
    else {
        panic!("expected generation-status payload");
    };
    assert_eq!(repo_id.as_str(), "repo-1");
    assert_eq!(revision_id.as_str(), "rev-1");
}

#[test]
fn rejects_generation_status_missing_repo_id() {
    let parsed = ParsedCommand::parse(["generation-status", "--revision-id", "rev-1"]);
    assert!(parsed.is_err());
    let Err(error) = parsed else {
        return;
    };
    assert_eq!(error.exit_code, EXIT_USAGE);
    assert!(error.message.contains("--repo-id"));
}

#[test]
fn rejects_generation_status_missing_revision_id() {
    let parsed = ParsedCommand::parse(["generation-status", "--repo-id", "repo-1"]);
    assert!(parsed.is_err());
    let Err(error) = parsed else {
        return;
    };
    assert_eq!(error.exit_code, EXIT_USAGE);
    assert!(error.message.contains("--revision-id"));
}

#[test]
fn process_readiness_has_no_repository_selector() {
    let parsed = ParsedCommand::parse(["readiness", "--output", "json"]);
    let Ok(parsed) = parsed else {
        panic!("process readiness parses");
    };
    assert_eq!(parsed.kind, CommandKind::Readiness);
    assert_eq!(parsed.request, CliRequest::ProcessReadiness);
    let with_repo = ParsedCommand::parse(["readiness", "--repo-id", "repo-1"]);
    assert!(with_repo.is_err());
}

#[test]
fn events_require_bounded_plane_and_limit() {
    let parsed = ParsedCommand::parse([
        "events", "--plane", "query", "--limit", "32", "--output", "json",
    ])
    .expect("bounded event request");
    assert_eq!(parsed.kind, CommandKind::RequestEvents);
    assert_eq!(
        parsed.request,
        CliRequest::RequestEvents {
            plane: ProcessRequestEventPlaneV1::Query,
            limit: 32,
        }
    );
    for args in [
        vec!["events", "--plane", "query"],
        vec!["events", "--plane", "query", "--limit", "0"],
        vec!["events", "--plane", "query", "--limit", "1025"],
        vec!["events", "--plane", "unknown", "--limit", "1"],
    ] {
        assert!(ParsedCommand::parse(args).is_err());
    }
    let invalid = ParsedCommand::parse(["events", "--plane", "query", "--limit", "abc"])
        .expect_err("non-numeric limit");
    assert_eq!(invalid.exit_code, EXIT_USAGE);
    assert!(invalid.message.contains("invalid digit found in string"));
}

#[test]
fn events_renderer_keeps_loss_and_process_identity_visible() {
    let events = ProcessRequestEventsV1 {
        process_instance: "0000000000000000000000000000002a".to_owned(),
        plane: ProcessRequestEventPlaneV1::Query,
        events: Vec::new(),
        oldest_retained_sequence: None,
        next_sequence: 1,
        dropped_before: 2,
        dropped_after: 2,
        omitted_before_window: false,
        sequence_exhausted: false,
    };
    let pretty = render_request_events(&events, OutputMode::Pretty).expect("pretty events");
    assert!(pretty.contains("process_instance: 0000000000000000000000000000002a"));
    assert!(pretty.contains("dropped_before: 2"));
    assert!(pretty.contains("omitted_before_window: false"));
    let json = render_request_events(&events, OutputMode::Json).expect("JSON events");
    let decoded: ProcessRequestEventsV1 = serde_json::from_str(&json).expect("typed JSON events");
    assert_eq!(decoded, events);
}

fn metrics_fixture() -> MetricsSnapshotV1 {
    use quanta_index_contract::{
        MetricBucketV1, MetricCounterV1, MetricGaugeV1, MetricsDiagnosticsV1,
    };
    MetricsSnapshotV1 {
        counters: vec![
            MetricCounterV1 {
                name: "ipc_query_requests_dispatched_total".to_string(),
                value: 13,
            },
            MetricCounterV1 {
                name: "lq_route_lexical_served_total".to_string(),
                value: 12,
            },
        ],
        gauges: vec![MetricGaugeV1 {
            name: "ipc_query_connections_live".to_string(),
            value: 1.0,
        }],
        histograms: vec![MetricHistogramV1 {
            name: "lq_route_lexical_latency_ms".to_string(),
            count: 3,
            sum: 8.5,
            min: 0.5,
            max: 7.0,
            buckets: vec![
                MetricBucketV1 { le: 1.0, count: 2 },
                MetricBucketV1 { le: 2.5, count: 2 },
                MetricBucketV1 { le: 10.0, count: 3 },
            ],
        }],
        diagnostics: MetricsDiagnosticsV1 {
            samples_recorded: 40,
            samples_dropped: 0,
            errors_recorded: 2,
            errors_dropped: 0,
        },
    }
}

/// QI-BB-015: the pretty rendering is line-oriented and complete.
#[test]
fn render_metrics_pretty_lists_every_series_and_the_diagnostics() {
    let rendered = render_metrics(&metrics_fixture(), OutputMode::Pretty);
    assert!(rendered.is_ok(), "{rendered:?}");
    let Ok(text) = rendered else {
        return;
    };
    assert_eq!(
        text,
        "kind: metrics\n\
             counters: 2\n\
             \x20 ipc_query_requests_dispatched_total 13\n\
             \x20 lq_route_lexical_served_total 12\n\
             gauges: 1\n\
             \x20 ipc_query_connections_live 1\n\
             histograms: 1\n\
             \x20 lq_route_lexical_latency_ms count=3 sum=8.5 min=0.5 max=7\n\
             \x20   le=1 2\n\
             \x20   le=2.5 2\n\
             \x20   le=10 3\n\
             \x20   le=+Inf 3\n\
             diagnostics: samples_recorded=40 samples_dropped=0 errors_recorded=2 errors_dropped=0\n"
    );
}

/// QI-BB-015: the Prometheus exposition is exactly what a scraper reads.
///
/// Typed families, cumulative `_bucket` series ending in a `+Inf` bucket
/// spelled from `count`, and the diagnostics as their own counters.
#[test]
fn render_metrics_prometheus_emits_the_text_exposition_format() {
    let rendered = render_metrics(&metrics_fixture(), OutputMode::Prometheus);
    assert!(rendered.is_ok(), "{rendered:?}");
    let Ok(text) = rendered else {
        return;
    };
    assert_eq!(
        text,
        "# TYPE ipc_query_requests_dispatched_total counter\n\
             ipc_query_requests_dispatched_total 13\n\
             # TYPE lq_route_lexical_served_total counter\n\
             lq_route_lexical_served_total 12\n\
             # TYPE ipc_query_connections_live gauge\n\
             ipc_query_connections_live 1\n\
             # TYPE lq_route_lexical_latency_ms histogram\n\
             lq_route_lexical_latency_ms_bucket{le=\"1\"} 2\n\
             lq_route_lexical_latency_ms_bucket{le=\"2.5\"} 2\n\
             lq_route_lexical_latency_ms_bucket{le=\"10\"} 3\n\
             lq_route_lexical_latency_ms_bucket{le=\"+Inf\"} 3\n\
             lq_route_lexical_latency_ms_sum 8.5\n\
             lq_route_lexical_latency_ms_count 3\n\
             # TYPE searchd_obs_samples_recorded_total counter\n\
             searchd_obs_samples_recorded_total 40\n\
             # TYPE searchd_obs_samples_dropped_total counter\n\
             searchd_obs_samples_dropped_total 0\n\
             # TYPE searchd_obs_errors_recorded_total counter\n\
             searchd_obs_errors_recorded_total 2\n\
             # TYPE searchd_obs_errors_dropped_total counter\n\
             searchd_obs_errors_dropped_total 0\n"
    );
}

/// QI-BB-015: `json` is the wire shape, decodable back into the snapshot.
#[test]
fn render_metrics_json_round_trips_the_snapshot() {
    let fixture = metrics_fixture();
    let rendered = render_metrics(&fixture, OutputMode::Json);
    assert!(rendered.is_ok(), "{rendered:?}");
    let Ok(text) = rendered else {
        return;
    };
    let decoded: Result<MetricsSnapshotV1, _> = serde_json::from_str(&text);
    assert!(decoded.is_ok(), "{decoded:?}");
    let Ok(decoded) = decoded else {
        return;
    };
    assert_eq!(decoded, fixture);
}

/// QI-BB-015: `metrics` takes only the global flags, and `prometheus`
/// output belongs to it alone.
#[test]
fn metrics_parses_alone_and_prometheus_output_is_refused_elsewhere() {
    let parsed = ParsedCommand::parse(["metrics", "--output", "prometheus"]);
    assert!(parsed.is_ok(), "{parsed:?}");
    let Ok(parsed) = parsed else {
        return;
    };
    assert_eq!(parsed.kind, CommandKind::Metrics);
    assert_eq!(parsed.output, OutputMode::Prometheus);
    assert_eq!(parsed.request, CliRequest::Metrics);

    let with_flag = ParsedCommand::parse(["metrics", "--repo-id", "repo"]);
    assert!(with_flag.is_err(), "{with_flag:?}");
    let Err(error) = with_flag else {
        return;
    };
    assert_eq!(error.exit_code, EXIT_USAGE);
    assert!(error.message.contains("unknown metrics flag `--repo-id`"));

    let readiness = ParsedCommand::parse(["readiness", "--output", "prometheus"]);
    assert!(readiness.is_err(), "{readiness:?}");
    let Err(error) = readiness else {
        return;
    };
    assert_eq!(error.exit_code, EXIT_USAGE);
    assert!(
        error
            .message
            .contains("renders only `metrics`, not `readiness`"),
        "{}",
        error.message
    );
}

/// QI-BB-026: `quarantine list` takes only the global flags; a discard
/// names one entry exactly and every incomplete or mixed form is a
/// usage error before anything reaches the socket.
#[test]
fn quarantine_parses_list_and_exact_discard_targets_only() {
    let parsed = ParsedCommand::parse(["quarantine", "list", "--output", "json"]);
    assert!(parsed.is_ok(), "{parsed:?}");
    let Ok(parsed) = parsed else {
        return;
    };
    assert_eq!(parsed.kind, CommandKind::Quarantine);
    assert_eq!(parsed.output, OutputMode::Json);
    assert_eq!(parsed.request, CliRequest::QuarantineList);

    let generation = ParsedCommand::parse([
        "quarantine",
        "discard",
        "--track",
        "semantic",
        "--path",
        "/state/indexes/semantic/repo/rev/g3",
        "--reason",
        "GENERATION_QUARANTINE_SCOPE_MISMATCH",
        "--detail",
        "identity names repo other",
    ]);
    assert!(generation.is_ok(), "{generation:?}");
    let Ok(generation) = generation else {
        return;
    };
    assert_eq!(
        generation.request,
        CliRequest::QuarantineDiscard(QuarantineTargetV1::Generation(
            QuarantinedGenerationEntryV1 {
                track: SearchPlaneTrackKind::Semantic,
                path: "/state/indexes/semantic/repo/rev/g3".to_string(),
                reason: "GENERATION_QUARANTINE_SCOPE_MISMATCH".to_string(),
                detail: "identity names repo other".to_string(),
            }
        ))
    );
    let without_detail = ParsedCommand::parse([
        "quarantine",
        "discard",
        "--track",
        "lexical",
        "--path",
        "/state/indexes/lexical/repo-legacy",
        "--reason",
        "GENERATION_QUARANTINE_NON_CANONICAL_LAYOUT",
    ]);
    assert!(without_detail.is_ok(), "{without_detail:?}");
    let Ok(without_detail) = without_detail else {
        return;
    };
    assert_eq!(
        without_detail.request,
        CliRequest::QuarantineDiscard(QuarantineTargetV1::Generation(
            QuarantinedGenerationEntryV1 {
                track: SearchPlaneTrackKind::Lexical,
                path: "/state/indexes/lexical/repo-legacy".to_string(),
                reason: "GENERATION_QUARANTINE_NON_CANONICAL_LAYOUT".to_string(),
                detail: String::new(),
            }
        ))
    );
    let repo_map = ParsedCommand::parse([
        "quarantine",
        "discard",
        "--repomap-file",
        "stale--marker.json",
        "--reason",
        "snapshot does not decode",
    ]);
    assert!(repo_map.is_ok(), "{repo_map:?}");
    let Ok(repo_map) = repo_map else {
        return;
    };
    assert_eq!(
        repo_map.request,
        CliRequest::QuarantineDiscard(QuarantineTargetV1::RepoMapFile(
            QuarantinedRepoMapFileEntryV1 {
                file_name: "stale--marker.json".to_string(),
                reason: "snapshot does not decode".to_string(),
            }
        ))
    );

    for (args, needle) in [
        (vec!["quarantine"], "requires `list` or `discard`"),
        (
            vec!["quarantine", "purge"],
            "unknown quarantine verb `purge`",
        ),
        (
            vec!["quarantine", "list", "--all"],
            "unknown quarantine list flag `--all`",
        ),
        (vec!["quarantine", "discard"], "needs --track and --path"),
        (
            vec![
                "quarantine",
                "discard",
                "--track",
                "lexical",
                "--path",
                "/p",
            ],
            "--path requires --reason",
        ),
        (
            vec![
                "quarantine",
                "discard",
                "--track",
                "lexical",
                "--reason",
                "R",
            ],
            "needs --track and --path",
        ),
        (
            vec!["quarantine", "discard", "--path", "/p", "--reason", "R"],
            "needs --track and --path",
        ),
        (
            vec![
                "quarantine",
                "discard",
                "--track",
                "repomap",
                "--path",
                "/p",
                "--reason",
                "R",
            ],
            "unsupported quarantine track `repomap`",
        ),
        (
            vec!["quarantine", "discard", "--repomap-file", "x.json"],
            "--repomap-file requires --reason",
        ),
        (
            vec![
                "quarantine",
                "discard",
                "--repomap-file",
                "x.json",
                "--track",
                "lexical",
                "--reason",
                "R",
            ],
            "cannot be combined",
        ),
        (
            vec!["quarantine", "discard", "--force"],
            "unknown quarantine discard flag `--force`",
        ),
        (
            vec!["quarantine", "list", "--output", "prometheus"],
            "renders only `metrics`, not `quarantine`",
        ),
    ] {
        let parsed = ParsedCommand::parse(args.clone());
        let Err(error) = parsed else {
            panic!("{args:?} must be a usage error, parsed {parsed:?}");
        };
        assert_eq!(error.exit_code, EXIT_USAGE, "{args:?}: {}", error.message);
        assert!(
            error.message.contains(needle),
            "{args:?}: {} lacks {needle:?}",
            error.message
        );
    }
}

/// QI-BB-026: the pretty inventory prints each entry as the discard
/// flags that name it, the JSON form is the wire DTO, and the ack
/// renders the target and outcome the daemon returned.
#[test]
fn render_quarantine_prints_discardable_lines_and_the_ack() {
    let inventory = QuarantineInventoryV1 {
        lexical: vec![QuarantinedGenerationEntryV1 {
            track: SearchPlaneTrackKind::Lexical,
            path: "/state/indexes/lexical/repo/rev/g9".to_string(),
            reason: "GENERATION_QUARANTINE_IDENTITY_UNREADABLE".to_string(),
            detail: "identity file does not decode".to_string(),
        }],
        semantic: Vec::new(),
        repo_map: vec![QuarantinedRepoMapFileEntryV1 {
            file_name: "stale--marker.json".to_string(),
            reason: "snapshot does not decode".to_string(),
        }],
    };
    let pretty = render_quarantine_inventory(&inventory, OutputMode::Pretty);
    let Ok(pretty) = pretty else {
        panic!("pretty inventory renders: {pretty:?}");
    };
    let expected = [
            "kind: quarantine",
            "lexical: 1",
            r#"  --track lexical --path /state/indexes/lexical/repo/rev/g9 --reason GENERATION_QUARANTINE_IDENTITY_UNREADABLE --detail "identity file does not decode""#,
            "semantic: 0",
            "repo_map: 1",
            r#"  --repomap-file stale--marker.json --reason "snapshot does not decode""#,
            "",
        ]
        .join("\n");
    assert_eq!(pretty, expected);
    let json = render_quarantine_inventory(&inventory, OutputMode::Json);
    let Ok(json) = json else {
        panic!("json inventory renders: {json:?}");
    };
    let Ok(decoded) = serde_json::from_str::<QuarantineInventoryV1>(&json) else {
        panic!("json inventory decodes back: {json}");
    };
    assert_eq!(decoded, inventory);
    let prometheus = render_quarantine_inventory(&inventory, OutputMode::Prometheus);
    let Err(error) = prometheus else {
        panic!("prometheus output is metrics-only: {prometheus:?}");
    };
    assert_eq!(error.exit_code, EXIT_USAGE);

    let ack = QuarantineDiscardAck {
        target: QuarantineTargetV1::RepoMapFile(QuarantinedRepoMapFileEntryV1 {
            file_name: "stale--marker.json".to_string(),
            reason: "snapshot does not decode".to_string(),
        }),
        outcome: QuarantineDiscardOutcomeDtoV1::Discarded { bytes: 15 },
    };
    let pretty = render_quarantine_discard(&ack, OutputMode::Pretty);
    let Ok(pretty) = pretty else {
        panic!("pretty ack renders: {pretty:?}");
    };
    assert_eq!(
        pretty,
        "kind: quarantine-discard\ntarget: repo_map stale--marker.json\noutcome: discarded bytes=15\n"
    );
    let absent = QuarantineDiscardAck {
        target: QuarantineTargetV1::Generation(QuarantinedGenerationEntryV1 {
            track: SearchPlaneTrackKind::Semantic,
            path: "/state/indexes/semantic/repo/rev/g1".to_string(),
            reason: "GENERATION_QUARANTINE_SCOPE_MISMATCH".to_string(),
            detail: String::new(),
        }),
        outcome: QuarantineDiscardOutcomeDtoV1::Absent,
    };
    let pretty = render_quarantine_discard(&absent, OutputMode::Pretty);
    let Ok(pretty) = pretty else {
        panic!("pretty ack renders: {pretty:?}");
    };
    assert_eq!(
        pretty,
        "kind: quarantine-discard\ntarget: semantic /state/indexes/semantic/repo/rev/g1\noutcome: absent\n"
    );
    let json = render_quarantine_discard(&absent, OutputMode::Json);
    let Ok(json) = json else {
        panic!("json ack renders: {json:?}");
    };
    let Ok(decoded) = serde_json::from_str::<QuarantineDiscardAck>(&json) else {
        panic!("json ack decodes back: {json}");
    };
    assert_eq!(decoded, absent);
}

#[test]
fn render_generation_status_json_emits_report_shape() {
    use quanta_index_contract::ipc::{
        GenerationStatusReport, SearchPlaneTrackKind, TrackReadinessRecord,
    };
    let report = GenerationStatusReport {
        repo_id: RepoId::new("repo-1").expect("static fixture ID satisfies canonical policy"),
        revision_id: RevisionId::new("rev-1")
            .expect("static fixture ID satisfies canonical policy"),
        semantic_content: None,
        tracks: vec![TrackReadinessRecord {
            track: SearchPlaneTrackKind::Lexical,
            manifest_generation: ManifestGeneration::new(11),
            manifest_digest: "digest-11".to_string(),
        }],
    };
    let rendered = render_generation_status(&report, OutputMode::Json);
    assert!(rendered.is_ok());
    let Ok(text) = rendered else {
        return;
    };
    let parsed = serde_json::from_str::<serde_json::Value>(&text);
    assert!(parsed.is_ok(), "json output must parse: {text}");
    assert!(text.contains("\"repo_id\": \"repo-1\""));
    assert!(text.contains("\"revision_id\": \"rev-1\""));
    assert!(text.contains("\"track\": \"Lexical\""));
    assert!(text.contains("\"manifest_digest\": \"digest-11\""));
    assert!(text.ends_with('\n'));
}

#[test]
fn render_generation_status_pretty_marks_empty_tracks() {
    use quanta_index_contract::ipc::GenerationStatusReport;
    let report = GenerationStatusReport {
        repo_id: RepoId::new("repo-1").expect("static fixture ID satisfies canonical policy"),
        revision_id: RevisionId::new("rev-1")
            .expect("static fixture ID satisfies canonical policy"),
        semantic_content: None,
        tracks: vec![],
    };
    let rendered = render_generation_status(&report, OutputMode::Pretty);
    assert!(rendered.is_ok());
    let Ok(text) = rendered else {
        return;
    };
    assert!(text.contains("kind: generation-status"));
    assert!(text.contains("tracks: 0 (none activated)"));
}

#[test]
fn process_readiness_renderer_keeps_component_failure_distinct_from_generation_status() {
    use quanta_index_contract::{
        ProcessComponentsHealthV1, ProcessProviderClaimV1, ProcessProviderReadinessV1,
        ProcessReadinessPhaseV1, ProcessReadinessReasonV1,
    };
    let report = ProcessReadinessV1 {
        ready: false,
        supervisor_phase: ProcessReadinessPhaseV1::Failed,
        components: ProcessComponentsHealthV1 {
            query_plane: false,
            control_plane: true,
            ingest_plane: true,
            maintenance_heartbeat: true,
            required_backend: true,
            provider: ProcessProviderReadinessV1 {
                claim: ProcessProviderClaimV1::Required,
                healthy: true,
            },
        },
        active_candidate_integrity: None,
        active_repositories: 0,
        not_ready_reasons: vec![
            ProcessReadinessReasonV1::SupervisorNotReady,
            ProcessReadinessReasonV1::QueryPlaneUnhealthy,
        ],
    };
    let pretty = render_process_readiness(&report, OutputMode::Pretty)
        .expect("well-formed process readiness renders");
    assert!(pretty.contains("kind: process-readiness"));
    assert!(pretty.contains("query_plane: false"));
    assert!(pretty.contains("not_ready: query_plane_unhealthy"));
    let json = render_process_readiness(&report, OutputMode::Json)
        .expect("well-formed process readiness serializes");
    assert!(json.contains("\"ready\": false"));
    assert!(json.contains("\"active_repositories\": 0"));
}

#[test]
fn render_generation_status_pretty_lists_tracks() {
    use quanta_index_contract::ipc::{
        GenerationStatusReport, SearchPlaneTrackKind, TrackReadinessRecord,
    };
    let report = GenerationStatusReport {
        repo_id: RepoId::new("repo-1").expect("static fixture ID satisfies canonical policy"),
        revision_id: RevisionId::new("rev-1")
            .expect("static fixture ID satisfies canonical policy"),
        semantic_content: None,
        tracks: vec![
            TrackReadinessRecord {
                track: SearchPlaneTrackKind::Lexical,
                manifest_generation: ManifestGeneration::new(11),
                manifest_digest: "lex".to_string(),
            },
            TrackReadinessRecord {
                track: SearchPlaneTrackKind::Semantic,
                manifest_generation: ManifestGeneration::new(12),
                manifest_digest: "sem".to_string(),
            },
        ],
    };
    let rendered = render_generation_status(&report, OutputMode::Pretty);
    assert!(rendered.is_ok());
    let Ok(text) = rendered else {
        return;
    };
    assert!(text.contains("tracks: 2"));
    assert!(text.contains("1. track=Lexical manifest_generation=11 manifest_digest=lex"));
    assert!(text.contains("2. track=Semantic manifest_generation=12 manifest_digest=sem"));
}

#[test]
fn after_publish_preserves_cause_exit_and_original_publication_for_both_stages() {
    use quanta_index_contract::{
        BatchPublishReceipt, GenerationSnapshot, SearchPlaneTrackKind, SourcePublicationBinding,
        SourcePublicationEvent,
    };
    let publication = SourcePublicationBinding {
        event: SourcePublicationEvent {
            stream_id: "publication-stream".into(),
            event_id: "publication-event".into(),
            expected_base_event_id: None,
            payload_sha256: [3; 32],
        },
        target: GenerationSnapshot {
            repo_id: RepoId::new("publication-repo").expect("repo"),
            revision_id: RevisionId::new("publication-revision").expect("revision"),
            track: SearchPlaneTrackKind::Lexical,
            manifest_generation: ManifestGeneration::new(7),
            manifest_digest: "publication-manifest".into(),
        },
        batch_digest: "c".repeat(64),
    };
    let receipt = BatchPublishReceipt::empty_for(
        publication.target.manifest_generation,
        Some(publication.target.manifest_digest.clone()),
        publication.batch_digest.clone(),
    )
    .recorded_at(7);
    let evidence = quanta_index_sdk::PublishedBatchEvidence {
        publication,
        receipt,
    };
    let cases: [(fn() -> SdkError, u8); 3] = [
        (
            || SdkError::Protocol("cause-protocol".into()),
            EXIT_PROTOCOL,
        ),
        (
            || SdkError::PlaneUnavailable { plane: "search" },
            EXIT_USAGE,
        ),
        (
            || SdkError::Remote {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::from_wire_str("QUERY_TIMEOUT")
                    .expect("known code"),
                message: "cause-remote".into(),
                repair: None,
            },
            EXIT_REMOTE,
        ),
    ];
    for stage in [
        quanta_index_sdk::PublishedBatchFailureStage::Observation,
        quanta_index_sdk::PublishedBatchFailureStage::Activation,
    ] {
        for (source, exit_code) in cases {
            let expected = map_sdk_error(source());
            let actual = map_sdk_error(SdkError::AfterPublish {
                stage,
                evidence: Box::new(evidence.clone()),
                source: Box::new(source()),
            });
            assert_eq!(actual.exit_code, exit_code);
            assert!(actual.message.starts_with(&expected.message));
            assert!(actual.message.contains(&format!("{stage:?}")));
            assert!(actual.message.contains("publication-revision"));
            assert!(actual.message.contains("publication-manifest"));
            assert!(actual.message.contains("durable_sequence: 7"));
        }
    }
}
