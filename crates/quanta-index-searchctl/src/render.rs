use crate::{
    CliError, CliResult, CommandKind, DoctorReport, OutputMode, prometheus_is_metrics_only,
};
use quanta_index_contract::{
    AuxEpochV1, ContinuationTokenV2, EarlyStopReason, EngineTouched, GenerationPin,
    HybridQueryResponse, HybridSeedQueryResponse, PlannerTraceEntry, ProcessReadinessV1,
    ProcessRequestEventsV1, QueryErrorRepair, QueryResultWindowV2, SearchExplanation,
    SearchPlaneHistoryQueryResponse, SearchPlaneQueryIpcResponse,
    SearchPlaneQueryIpcResponseEnvelope, SearchPlaneRuntimeMetadataQueryResponse,
    SearchPlaneStructuralQueryResponse, SymbolCandidate, SymbolQueryResponse, TextQueryResponse,
    TextRankUnit,
    ipc::{
        GenerationStatusReport, MetricHistogramV1, MetricsSnapshotV1, QuarantineDiscardAck,
        QuarantineDiscardOutcomeDtoV1, QuarantineInventoryV1, QuarantineTargetV1,
    },
};
use quanta_index_sdk::{PublishedBatchEvidence, PublishedBatchFailureStage};
use std::fmt::Write as _;
use std::io::Write;

pub(super) fn validate_response_kind(
    expected: CommandKind,
    response: &SearchPlaneQueryIpcResponseEnvelope,
) -> CliResult<()> {
    match (&expected, &response.payload) {
        (_, SearchPlaneQueryIpcResponse::Error(error)) => Err(CliError::remote(format!(
            "{}: {}",
            error.code, error.message
        ))),
        (CommandKind::Lexical, SearchPlaneQueryIpcResponse::Text(_))
        | (CommandKind::Symbol, SearchPlaneQueryIpcResponse::Symbol(_))
        | (CommandKind::Semantic, SearchPlaneQueryIpcResponse::Semantic(_))
        | (CommandKind::Hybrid, SearchPlaneQueryIpcResponse::Hybrid(_))
        | (CommandKind::HybridSeed, SearchPlaneQueryIpcResponse::HybridSeed(_))
        | (CommandKind::Explain, SearchPlaneQueryIpcResponse::Explain(_))
        | (CommandKind::RepoMap, SearchPlaneQueryIpcResponse::RepoMapQuery(_))
        | (CommandKind::RuntimeMetadata, SearchPlaneQueryIpcResponse::RuntimeMetadata(_))
        | (CommandKind::History, SearchPlaneQueryIpcResponse::History(_))
        | (CommandKind::Structural, SearchPlaneQueryIpcResponse::Structural(_)) => Ok(()),
        _ => Err(CliError::protocol(format!(
            "response kind `{}` does not match requested command `{}`",
            response_kind_name(&response.payload),
            command_kind_name(expected)
        ))),
    }
}

pub(super) fn render_response(
    output: OutputMode,
    response: &SearchPlaneQueryIpcResponseEnvelope,
    stdout: &mut dyn Write,
) -> CliResult<()> {
    match output {
        OutputMode::Json => {
            serde_json::to_writer_pretty(&mut *stdout, response).map_err(|err| {
                CliError::protocol(format!("failed to encode json output: {err}"))
            })?;
            stdout
                .write_all(b"\n")
                .map_err(|err| CliError::transport(format!("failed writing stdout: {err}")))?;
            Ok(())
        }
        OutputMode::Pretty => {
            let mut rendered = String::new();
            render_pretty(response, &mut rendered)?;
            stdout
                .write_all(rendered.as_bytes())
                .map_err(|err| CliError::transport(format!("failed writing stdout: {err}")))?;
            Ok(())
        }
        OutputMode::Prometheus => Err(prometheus_is_metrics_only("a query command")),
    }
}

/// J7Q-05: render a control-plane [`GenerationStatusReport`] to a string.
///
/// `json` mode serializes the report directly (the DTO carries a manual
/// `Serialize`), so the machine surface is the wire shape verbatim. `pretty`
/// mode emits a stable, line-oriented form: a header line plus one line per
/// activated track in `tracks` declaration order. An empty `tracks` vec renders
/// an explicit `tracks: 0 (none activated)` line — never a silent blank — to
/// keep "nothing activated" distinct from a malformed report.
pub(super) fn render_generation_status(
    report: &GenerationStatusReport,
    output: OutputMode,
) -> CliResult<String> {
    match output {
        OutputMode::Json => serde_json::to_string_pretty(report)
            .map(|mut text| {
                text.push('\n');
                text
            })
            .map_err(|err| CliError::protocol(format!("failed to encode json output: {err}"))),
        OutputMode::Prometheus => Err(prometheus_is_metrics_only("generation-status")),
        OutputMode::Pretty => {
            let mut rendered = String::new();
            fmt_ok(writeln!(rendered, "kind: generation-status"))?;
            fmt_ok(writeln!(
                rendered,
                "generation_status: repo_id={} revision_id={}",
                report.repo_id.as_str(),
                report.revision_id.as_str()
            ))?;
            if report.tracks.is_empty() {
                fmt_ok(writeln!(rendered, "tracks: 0 (none activated)"))?;
                return Ok(rendered);
            }
            fmt_ok(writeln!(rendered, "tracks: {}", report.tracks.len()))?;
            for (index, record) in report.tracks.iter().enumerate() {
                let display_index = index.checked_add(1).ok_or_else(|| {
                    CliError::protocol("generation-status track index overflow".to_string())
                })?;
                fmt_ok(writeln!(
                    rendered,
                    "{}. track={} manifest_generation={} manifest_digest={}",
                    display_index,
                    record.track.as_code_str(),
                    record.manifest_generation.get(),
                    record.manifest_digest
                ))?;
            }
            // The semantic content roots the active pair was activated
            // under (QI-BB-028): what the plane sealed, not what the
            // producer asked for.
            if let Some(roots) = &report.semantic_content {
                fmt_ok(writeln!(
                    rendered,
                    "semantic_content: row_root={} membership_root={}",
                    roots.row_root_digest, roots.membership_root_digest
                ))?;
            }
            Ok(rendered)
        }
    }
}

pub(super) fn render_process_readiness(
    report: &ProcessReadinessV1,
    output: OutputMode,
) -> CliResult<String> {
    match output {
        OutputMode::Json => serde_json::to_string_pretty(report)
            .map(|mut text| {
                text.push('\n');
                text
            })
            .map_err(|error| CliError::protocol(format!("failed to encode json output: {error}"))),
        OutputMode::Prometheus => Err(prometheus_is_metrics_only("readiness")),
        OutputMode::Pretty => {
            let mut rendered = String::new();
            fmt_ok(writeln!(rendered, "kind: process-readiness"))?;
            fmt_ok(writeln!(rendered, "ready: {}", report.ready))?;
            fmt_ok(writeln!(
                rendered,
                "supervisor_phase: {}",
                report.supervisor_phase.as_code_str()
            ))?;
            fmt_ok(writeln!(
                rendered,
                "active_repositories: {}",
                report.active_repositories
            ))?;
            fmt_ok(writeln!(
                rendered,
                "active_candidate_integrity: {}",
                report
                    .active_candidate_integrity
                    .map_or("not_applicable", |healthy| {
                        if healthy { "true" } else { "false" }
                    })
            ))?;
            fmt_ok(writeln!(
                rendered,
                "query_plane: {}",
                report.components.query_plane
            ))?;
            fmt_ok(writeln!(
                rendered,
                "control_plane: {}",
                report.components.control_plane
            ))?;
            fmt_ok(writeln!(
                rendered,
                "ingest_plane: {}",
                report.components.ingest_plane
            ))?;
            fmt_ok(writeln!(
                rendered,
                "maintenance_heartbeat: {}",
                report.components.maintenance_heartbeat
            ))?;
            fmt_ok(writeln!(
                rendered,
                "required_backend: {}",
                report.components.required_backend
            ))?;
            fmt_ok(writeln!(
                rendered,
                "provider_claim: {}",
                report.components.provider.claim.as_code_str()
            ))?;
            fmt_ok(writeln!(
                rendered,
                "provider_healthy: {}",
                report.components.provider.healthy
            ))?;
            for reason in &report.not_ready_reasons {
                fmt_ok(writeln!(rendered, "not_ready: {}", reason.as_code_str()))?;
            }
            Ok(rendered)
        }
    }
}

pub(super) fn render_request_events(
    events: &ProcessRequestEventsV1,
    output: OutputMode,
) -> CliResult<String> {
    match output {
        OutputMode::Json => serde_json::to_string_pretty(events)
            .map(|mut text| {
                text.push('\n');
                text
            })
            .map_err(|error| CliError::protocol(format!("failed to encode event JSON: {error}"))),
        OutputMode::Prometheus => Err(prometheus_is_metrics_only("events")),
        OutputMode::Pretty => {
            let mut rendered = String::new();
            fmt_ok(writeln!(rendered, "kind: process-request-events-v1"))?;
            fmt_ok(writeln!(
                rendered,
                "process_instance: {}",
                events.process_instance
            ))?;
            fmt_ok(writeln!(rendered, "plane: {}", events.plane.as_code_str()))?;
            fmt_ok(writeln!(
                rendered,
                "oldest_retained_sequence: {:?}",
                events.oldest_retained_sequence
            ))?;
            fmt_ok(writeln!(
                rendered,
                "next_sequence: {}",
                events.next_sequence
            ))?;
            fmt_ok(writeln!(
                rendered,
                "dropped_before: {}",
                events.dropped_before
            ))?;
            fmt_ok(writeln!(
                rendered,
                "dropped_after: {}",
                events.dropped_after
            ))?;
            fmt_ok(writeln!(
                rendered,
                "omitted_before_window: {}",
                events.omitted_before_window
            ))?;
            fmt_ok(writeln!(
                rendered,
                "sequence_exhausted: {}",
                events.sequence_exhausted
            ))?;
            for event in &events.events {
                fmt_ok(writeln!(
                    rendered,
                    "sequence={} request_id={} connection_id={} stage={} elapsed_micros={} route={:?} error={:?} ticket_id={:?} window_ordinal={:?}",
                    event.sequence,
                    event.request_id,
                    event.connection_id,
                    event.stage.as_code_str(),
                    event.elapsed_micros,
                    event.route,
                    event.error,
                    event.ticket_id,
                    event.window_ordinal,
                ))?;
            }
            Ok(rendered)
        }
    }
}

/// QI-BB-015: render a [`MetricsSnapshotV1`].
///
/// `json` is the wire shape verbatim. `pretty` is line-oriented: one line
/// per counter and gauge, a header plus one bucket line per histogram, and
/// the diagnostic tallies last. `prometheus` is the text exposition format:
/// a `# TYPE` line per metric, `_bucket{le="…"}` / `_sum` / `_count` series
/// per histogram. The wire carries only finite bounds, so both renderers
/// spell the `+Inf` bucket from the histogram's `count`.
pub(super) fn render_metrics(
    snapshot: &MetricsSnapshotV1,
    output: OutputMode,
) -> CliResult<String> {
    match output {
        OutputMode::Json => serde_json::to_string_pretty(snapshot)
            .map(|mut text| {
                text.push('\n');
                text
            })
            .map_err(|err| CliError::protocol(format!("failed to encode json output: {err}"))),
        OutputMode::Pretty => render_metrics_pretty(snapshot),
        OutputMode::Prometheus => render_metrics_prometheus(snapshot),
    }
}

fn render_metrics_pretty(snapshot: &MetricsSnapshotV1) -> CliResult<String> {
    let mut rendered = String::new();
    fmt_ok(writeln!(rendered, "kind: metrics"))?;
    fmt_ok(writeln!(rendered, "counters: {}", snapshot.counters.len()))?;
    for counter in &snapshot.counters {
        fmt_ok(writeln!(rendered, "  {} {}", counter.name, counter.value))?;
    }
    fmt_ok(writeln!(rendered, "gauges: {}", snapshot.gauges.len()))?;
    for gauge in &snapshot.gauges {
        fmt_ok(writeln!(rendered, "  {} {}", gauge.name, gauge.value))?;
    }
    fmt_ok(writeln!(
        rendered,
        "histograms: {}",
        snapshot.histograms.len()
    ))?;
    for histogram in &snapshot.histograms {
        fmt_ok(writeln!(
            rendered,
            "  {} count={} sum={} min={} max={}",
            histogram.name, histogram.count, histogram.sum, histogram.min, histogram.max
        ))?;
        for bucket in &histogram.buckets {
            fmt_ok(writeln!(rendered, "    le={} {}", bucket.le, bucket.count))?;
        }
        fmt_ok(writeln!(rendered, "    le=+Inf {}", histogram.count))?;
    }
    fmt_ok(writeln!(
        rendered,
        "diagnostics: samples_recorded={} samples_dropped={} errors_recorded={} errors_dropped={}",
        snapshot.diagnostics.samples_recorded,
        snapshot.diagnostics.samples_dropped,
        snapshot.diagnostics.errors_recorded,
        snapshot.diagnostics.errors_dropped
    ))?;
    Ok(rendered)
}

fn render_metrics_prometheus(snapshot: &MetricsSnapshotV1) -> CliResult<String> {
    let mut rendered = String::new();
    for counter in &snapshot.counters {
        fmt_ok(writeln!(rendered, "# TYPE {} counter", counter.name))?;
        fmt_ok(writeln!(rendered, "{} {}", counter.name, counter.value))?;
    }
    for gauge in &snapshot.gauges {
        fmt_ok(writeln!(rendered, "# TYPE {} gauge", gauge.name))?;
        fmt_ok(writeln!(rendered, "{} {}", gauge.name, gauge.value))?;
    }
    for histogram in &snapshot.histograms {
        render_prometheus_histogram(&mut rendered, histogram)?;
    }
    for (name, value) in [
        (
            "searchd_obs_samples_recorded_total",
            snapshot.diagnostics.samples_recorded,
        ),
        (
            "searchd_obs_samples_dropped_total",
            snapshot.diagnostics.samples_dropped,
        ),
        (
            "searchd_obs_errors_recorded_total",
            snapshot.diagnostics.errors_recorded,
        ),
        (
            "searchd_obs_errors_dropped_total",
            snapshot.diagnostics.errors_dropped,
        ),
    ] {
        fmt_ok(writeln!(rendered, "# TYPE {name} counter"))?;
        fmt_ok(writeln!(rendered, "{name} {value}"))?;
    }
    Ok(rendered)
}

fn render_prometheus_histogram(
    rendered: &mut String,
    histogram: &MetricHistogramV1,
) -> CliResult<()> {
    fmt_ok(writeln!(rendered, "# TYPE {} histogram", histogram.name))?;
    for bucket in &histogram.buckets {
        fmt_ok(writeln!(
            rendered,
            "{}_bucket{{le=\"{}\"}} {}",
            histogram.name, bucket.le, bucket.count
        ))?;
    }
    fmt_ok(writeln!(
        rendered,
        "{}_bucket{{le=\"+Inf\"}} {}",
        histogram.name, histogram.count
    ))?;
    fmt_ok(writeln!(
        rendered,
        "{}_sum {}",
        histogram.name, histogram.sum
    ))?;
    fmt_ok(writeln!(
        rendered,
        "{}_count {}",
        histogram.name, histogram.count
    ))?;
    Ok(())
}

/// QI-BB-026: render a [`QuarantineInventoryV1`].
///
/// `pretty` prints each entry on one line exactly as `quarantine discard`
/// wants it back, so an operator can copy a line into a discard.
pub(super) fn render_quarantine_inventory(
    inventory: &QuarantineInventoryV1,
    output: OutputMode,
) -> CliResult<String> {
    match output {
        OutputMode::Json => serde_json::to_string_pretty(inventory)
            .map(|mut text| {
                text.push('\n');
                text
            })
            .map_err(|err| CliError::protocol(format!("failed to encode json output: {err}"))),
        OutputMode::Prometheus => Err(prometheus_is_metrics_only("quarantine")),
        OutputMode::Pretty => {
            let mut rendered = String::new();
            fmt_ok(writeln!(rendered, "kind: quarantine"))?;
            for (label, entries) in [
                ("lexical", &inventory.lexical),
                ("semantic", &inventory.semantic),
            ] {
                fmt_ok(writeln!(rendered, "{label}: {}", entries.len()))?;
                for entry in entries {
                    fmt_ok(writeln!(
                        rendered,
                        "  --track {label} --path {} --reason {} --detail {:?}",
                        entry.path, entry.reason, entry.detail
                    ))?;
                }
            }
            fmt_ok(writeln!(rendered, "repo_map: {}", inventory.repo_map.len()))?;
            for entry in &inventory.repo_map {
                fmt_ok(writeln!(
                    rendered,
                    "  --repomap-file {} --reason {:?}",
                    entry.file_name, entry.reason
                ))?;
            }
            Ok(rendered)
        }
    }
}

/// QI-BB-026: render a [`QuarantineDiscardAck`].
pub(super) fn render_quarantine_discard(
    ack: &QuarantineDiscardAck,
    output: OutputMode,
) -> CliResult<String> {
    match output {
        OutputMode::Json => serde_json::to_string_pretty(ack)
            .map(|mut text| {
                text.push('\n');
                text
            })
            .map_err(|err| CliError::protocol(format!("failed to encode json output: {err}"))),
        OutputMode::Prometheus => Err(prometheus_is_metrics_only("quarantine")),
        OutputMode::Pretty => {
            let mut rendered = String::new();
            fmt_ok(writeln!(rendered, "kind: quarantine-discard"))?;
            let target = match &ack.target {
                QuarantineTargetV1::Generation(entry) => {
                    format!(
                        "{} {}",
                        entry.track.as_code_str().to_ascii_lowercase(),
                        entry.path
                    )
                }
                QuarantineTargetV1::RepoMapFile(entry) => format!("repo_map {}", entry.file_name),
            };
            let outcome = match ack.outcome {
                QuarantineDiscardOutcomeDtoV1::Discarded { bytes } => {
                    format!("discarded bytes={bytes}")
                }
                QuarantineDiscardOutcomeDtoV1::Absent => "absent".to_string(),
            };
            fmt_ok(writeln!(rendered, "target: {target}"))?;
            fmt_ok(writeln!(rendered, "outcome: {outcome}"))?;
            Ok(rendered)
        }
    }
}

/// J7Q-05: render a [`DoctorReport`].
///
/// `json` mode emits a stable machine-readable object (the field names are the
/// scriptable contract). `pretty` mode emits a line-oriented form. Both carry
/// the same verdict so automation and humans never reach different conclusions.
pub(super) fn render_doctor(report: &DoctorReport, output: OutputMode) -> CliResult<String> {
    match output {
        OutputMode::Json => serde_json::to_string_pretty(&doctor_report_to_json(report))
            .map(|mut text| {
                text.push('\n');
                text
            })
            .map_err(|err| CliError::protocol(format!("failed to encode json output: {err}"))),
        OutputMode::Prometheus => Err(prometheus_is_metrics_only("doctor")),
        OutputMode::Pretty => {
            let mut rendered = String::new();
            fmt_ok(writeln!(rendered, "kind: doctor"))?;
            fmt_ok(writeln!(
                rendered,
                "generation_status: repo_id={} revision_id={}",
                report.repo_id, report.revision_id
            ))?;
            fmt_ok(writeln!(rendered, "serve_ready: {}", report.serve_ready))?;
            fmt_ok(writeln!(
                rendered,
                "all_resolvable: {}",
                report.all_resolvable
            ))?;
            if report.tracks.is_empty() {
                fmt_ok(writeln!(rendered, "tracks: 0 (none activated)"))?;
                return Ok(rendered);
            }
            fmt_ok(writeln!(rendered, "tracks: {}", report.tracks.len()))?;
            for (index, track) in report.tracks.iter().enumerate() {
                let display_index = index
                    .checked_add(1)
                    .ok_or_else(|| CliError::protocol("doctor track index overflow".to_string()))?;
                fmt_ok(writeln!(
                    rendered,
                    "{}. track={} manifest_generation={} manifest_digest={} resolver_ok={}",
                    display_index,
                    track.track.as_code_str(),
                    track.manifest_generation,
                    track.manifest_digest,
                    track.resolver_ok
                ))?;
                fmt_ok(writeln!(rendered, "   resolver: {}", track.resolver_note))?;
            }
            Ok(rendered)
        }
    }
}

/// Stable JSON shape for [`DoctorReport`].
///
/// Field names here are the scriptable contract — automation reads `serve_ready`
/// / `all_resolvable` / per-track `resolver_ok` directly. Built as a value (not a
/// serde-derived DTO) because it is a CLI-side composite of two wire DTOs, not
/// itself a wire DTO.
fn doctor_report_to_json(report: &DoctorReport) -> serde_json::Value {
    let tracks = report
        .tracks
        .iter()
        .map(|track| {
            serde_json::json!({
                "track": track.track.as_code_str(),
                "manifest_generation": track.manifest_generation,
                "manifest_digest": track.manifest_digest,
                "resolver_ok": track.resolver_ok,
                "resolver_note": track.resolver_note,
            })
        })
        .collect::<Vec<_>>();
    serde_json::json!({
        "kind": "doctor",
        "repo_id": report.repo_id,
        "revision_id": report.revision_id,
        "serve_ready": report.serve_ready,
        "all_resolvable": report.all_resolvable,
        "track_count": report.tracks.len(),
        "tracks": tracks,
    })
}

fn render_pretty(
    response: &SearchPlaneQueryIpcResponseEnvelope,
    rendered: &mut String,
) -> CliResult<()> {
    fmt_ok(writeln!(rendered, "request_id: {}", response.request_id))?;
    match &response.payload {
        SearchPlaneQueryIpcResponse::Text(payload) => {
            render_lexical_payload("lexical", payload, Some(&payload.explanation), rendered)
        }
        SearchPlaneQueryIpcResponse::Symbol(payload) => render_symbol_payload(payload, rendered),
        SearchPlaneQueryIpcResponse::Semantic(payload) => render_lexical_payload(
            "semantic",
            &TextQueryResponse {
                selected_active_head: None,
                explanation: quanta_index_contract::SearchExplanation::empty(),
                generation: payload.generation.clone(),
                rank_unit: TextRankUnit::Chunk,
                results: payload.results.clone(),
                window: payload.window.clone(),
                file_owner_rows: None,
                next_cursor: None,
            },
            Some(&payload.explanation),
            rendered,
        ),
        SearchPlaneQueryIpcResponse::SemanticWorkBoundedV1(_) => Err(CliError::protocol(
            "CLI does not issue work-bounded semantic queries".to_string(),
        )),
        SearchPlaneQueryIpcResponse::Hybrid(payload) => render_hybrid_payload(payload, rendered),
        SearchPlaneQueryIpcResponse::HybridSeed(payload) => {
            render_hybrid_seed_payload(payload, rendered)
        }
        SearchPlaneQueryIpcResponse::Explain(payload) => {
            fmt_ok(writeln!(rendered, "kind: explain"))?;
            render_generation(&payload.generation, rendered)?;
            fmt_ok(writeln!(
                rendered,
                "presence: {}",
                payload.presence.as_str()
            ))?;
            render_explanation(&payload.explanation, rendered)?;
            Ok(())
        }
        SearchPlaneQueryIpcResponse::RepoMapQuery(payload) => {
            fmt_ok(writeln!(rendered, "kind: repomap"))?;
            fmt_ok(writeln!(
                rendered,
                "generation: repo_id={} revision_id={} manifest_generation={}",
                payload.repo_id.as_str(),
                payload.revision_id.as_str(),
                payload.manifest_generation.get()
            ))?;
            let meta = &payload.snapshot_meta;
            fmt_ok(writeln!(
                rendered,
                "snapshot: id={} projection_version={} authority_digest={}",
                meta.snapshot_id, meta.projection_version, meta.authority_digest
            ))?;
            fmt_ok(writeln!(
                rendered,
                "entries: {} dropped_entries_count={} drop_reason_codes={} degraded_reason_codes={}",
                payload.entries.len(),
                payload.dropped_entries_count,
                payload.drop_reason_codes.join(","),
                payload.degraded_reason_codes.join(",")
            ))?;
            for (index, entry) in payload.entries.iter().enumerate() {
                let display_index = index.checked_add(1).ok_or_else(|| {
                    CliError::protocol("repomap entry index overflow".to_string())
                })?;
                fmt_ok(writeln!(
                    rendered,
                    "{}. subject_identity={} doc_type={} kind={} owner_path={} rank={} score={} final_score_millis={}",
                    display_index,
                    entry.subject_identity,
                    entry.subject_doc_type.as_code_str(),
                    entry.subject_kind,
                    entry.owner_path,
                    entry.rank,
                    entry.score,
                    entry.final_score_millis
                ))?;
                fmt_ok(writeln!(
                    rendered,
                    "   authority: artifact_id={} digest={} projection_status={} redaction_state={}",
                    entry.projection_authority_artifact_id,
                    entry.projection_authority_digest,
                    entry.projection_status,
                    entry.redaction_state.as_code_str()
                ))?;
                if !entry.contributing_signals.is_empty() {
                    let pairs = entry
                        .contributing_signals
                        .iter()
                        .map(|(name, value)| format!("{name}={value}"))
                        .collect::<Vec<_>>()
                        .join(", ");
                    fmt_ok(writeln!(rendered, "   contributing_signals: {pairs}"))?;
                }
            }
            Ok(())
        }
        SearchPlaneQueryIpcResponse::Error(error) => Err(CliError::remote(
            render_remote_error_text(&error.code, &error.message, error.repair.as_ref())?,
        )),
        SearchPlaneQueryIpcResponse::RuntimeMetadata(payload) => {
            render_runtime_metadata_payload(payload, rendered)
        }
        SearchPlaneQueryIpcResponse::History(payload) => render_history_payload(payload, rendered),
        SearchPlaneQueryIpcResponse::Structural(payload) => {
            render_structural_payload(payload, rendered)
        }
        SearchPlaneQueryIpcResponse::ActiveGenerationSnapshot(_)
        | SearchPlaneQueryIpcResponse::ResolvedLexicalGeneration(_)
        | SearchPlaneQueryIpcResponse::ClusterMembershipRead(_) => {
            Err(CliError::protocol(format!(
                "{} is an SDK authority response and has no searchctl command",
                response_kind_name(&response.payload)
            )))
        }
    }
}

/// Render a hybrid response (QI-BB-022).
///
/// One line per fused row: the lane row as a lexical hit, then the RRF
/// score and each lane's rank and raw score.
fn render_hybrid_payload(payload: &HybridQueryResponse, rendered: &mut String) -> CliResult<()> {
    fmt_ok(writeln!(rendered, "kind: hybrid"))?;
    render_generation(&payload.generation, rendered)?;
    fmt_ok(writeln!(rendered, "results: {}", payload.results.len()))?;
    render_window_line(&payload.window, rendered)?;
    for (index, row) in payload.results.iter().enumerate() {
        let display_index = index
            .checked_add(1)
            .ok_or_else(|| CliError::protocol("candidate index overflow".to_string()))?;
        let lanes = row
            .contributions
            .iter()
            .map(|contribution| {
                format!(
                    "{}#{}({})",
                    contribution.lane.as_code_str(),
                    contribution.rank,
                    contribution.raw_score
                )
            })
            .collect::<Vec<_>>()
            .join(" ");
        fmt_ok(writeln!(
            rendered,
            "{}. candidate_id={} path={} lines={}-{} score={} fused={} lanes={}",
            display_index,
            row.candidate.candidate_id,
            row.candidate.repo_relative_path.as_str(),
            row.candidate.start_line,
            row.candidate.end_line,
            row.candidate.score,
            row.fused_score,
            lanes
        ))?;
        for line in row.candidate.snippet.lines() {
            fmt_ok(writeln!(rendered, "   {line}"))?;
        }
    }
    render_explanation(&payload.explanation, rendered)?;
    Ok(())
}

/// Render the one canonical seed list (QI-BB-019): each seed with its
/// typed identity and the lane contributions that ranked it.
fn render_hybrid_seed_payload(
    payload: &HybridSeedQueryResponse,
    rendered: &mut String,
) -> CliResult<()> {
    fmt_ok(writeln!(rendered, "kind: hybrid-seed"))?;
    render_generation(&payload.generation, rendered)?;
    fmt_ok(writeln!(
        rendered,
        "manifest_digest: {} seeds: {} has_more: {}",
        payload.manifest_digest,
        payload.window.returned(),
        display_has_more(payload.window.has_more())
    ))?;
    for seed in &payload.seed_candidates {
        let contributions = seed
            .contributions
            .iter()
            .map(|contribution| {
                let lane = match contribution.lane {
                    quanta_index_contract::SeedLane::Exact => "exact",
                    quanta_index_contract::SeedLane::Bm25 => "bm25",
                    quanta_index_contract::SeedLane::Dense => "dense",
                };
                contribution.corpus_kind.map_or_else(
                    || format!("{lane}#{}", contribution.rank),
                    |corpus| format!("{lane}#{}@{}", contribution.rank, corpus.as_code_str()),
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        fmt_ok(writeln!(
            rendered,
            "{}. entity={} owner_kind={} path={} lanes={}{}",
            seed.seed_rank,
            seed.entity_id,
            seed.owner_kind.as_code_str(),
            seed.repo_relative_path.as_str(),
            contributions,
            if seed.degraded_reasons.is_empty() {
                String::new()
            } else {
                format!(" degraded={}", seed.degraded_reasons.join(","))
            }
        ))?;
        for line in seed.snippet.lines().take(3) {
            fmt_ok(writeln!(rendered, "   {line}"))?;
        }
    }
    render_explanation(&payload.explanation, rendered)?;
    Ok(())
}

fn render_history_payload(
    payload: &SearchPlaneHistoryQueryResponse,
    rendered: &mut String,
) -> CliResult<()> {
    fmt_ok(writeln!(rendered, "kind: history"))?;
    render_generation(&payload.generation, rendered)?;
    fmt_ok(writeln!(
        rendered,
        "commits: {} diffs: {}",
        payload.commits.len(),
        payload.diffs.len()
    ))?;
    let matched = match payload.window.candidate_count() {
        quanta_index_contract::CandidateCountV1::Exact(count) => format!("{count}"),
        quanta_index_contract::CandidateCountV1::AtLeast(count) => format!(">={count}"),
    };
    fmt_ok(writeln!(
        rendered,
        "order: {} matched: {matched} examined: {} has_more: {}",
        payload.order,
        payload.examined,
        display_has_more(payload.window.has_more())
    ))?;
    render_read_epoch_line(payload.read_epoch, rendered)?;
    render_continuation_cursor(payload.next_cursor.as_ref(), rendered)?;
    for (index, commit) in payload.commits.iter().enumerate() {
        let display_index = index
            .checked_add(1)
            .ok_or_else(|| CliError::protocol("commit index overflow".to_string()))?;
        fmt_ok(writeln!(
            rendered,
            "{}.{} sha={} author={} committer={} committed_at_unix_s={} is_merge={} tags={}",
            display_index,
            render_history_score(commit.score),
            commit.sha.to_hex(),
            commit.author,
            commit.committer,
            commit.committed_at_unix_s,
            commit.is_merge,
            commit.tags.join(",")
        ))?;
        for line in commit.message.lines() {
            fmt_ok(writeln!(rendered, "   {line}"))?;
        }
    }
    for (index, diff) in payload.diffs.iter().enumerate() {
        let display_index = index
            .checked_add(1)
            .ok_or_else(|| CliError::protocol("diff index overflow".to_string()))?;
        fmt_ok(writeln!(
            rendered,
            "{}.{} path={} hunk_header={} side={} lines={}-{}",
            display_index,
            render_history_score(diff.score),
            diff.repo_relative_path,
            diff.hunk_header,
            diff.side.as_str(),
            diff.line_start,
            diff.line_end
        ))?;
        for line in diff.snippet.lines() {
            fmt_ok(writeln!(rendered, "   {line}"))?;
        }
    }
    Ok(())
}

/// ` score=<s>` for a scored row (a relevance page), nothing otherwise.
fn render_history_score(score: Option<quanta_index_contract::HistoryScoreV1>) -> String {
    score.map_or_else(String::new, |score| format!(" score={score}"))
}

/// One line for a page's window: how many candidates matched (exactly, or
/// at least) and whether the page cut them (QI-BB-025).
fn display_has_more(value: Option<bool>) -> &'static str {
    match value {
        Some(true) => "true",
        Some(false) => "false",
        None => "unknown",
    }
}

fn render_window_line(window: &QueryResultWindowV2, rendered: &mut String) -> CliResult<()> {
    let matched = match window.candidate_count() {
        quanta_index_contract::CandidateCountV1::Exact(count) => format!("{count}"),
        quanta_index_contract::CandidateCountV1::AtLeast(count) => format!(">={count}"),
    };
    fmt_ok(writeln!(
        rendered,
        "matched: {matched} has_more: {}",
        display_has_more(window.has_more())
    ))
}

/// One line for the auxiliary authority epoch a page was cut from
/// (QI-BB-020 W2); JSON output carries it as `read_epoch`.
fn render_read_epoch_line(epoch: AuxEpochV1, rendered: &mut String) -> CliResult<()> {
    fmt_ok(writeln!(rendered, "epoch: {epoch}"))
}

/// One line for a keyset page's order, window and work (QI-BB-025 W4),
/// in the shape the history page prints: `order: candidate_id matched: N
/// examined: M has_more: B`.
fn render_keyset_page_line(
    window: &QueryResultWindowV2,
    examined: u64,
    rendered: &mut String,
) -> CliResult<()> {
    let matched = match window.candidate_count() {
        quanta_index_contract::CandidateCountV1::Exact(count) => format!("{count}"),
        quanta_index_contract::CandidateCountV1::AtLeast(count) => format!(">={count}"),
    };
    fmt_ok(writeln!(
        rendered,
        "order: candidate_id matched: {matched} examined: {examined} has_more: {}",
        display_has_more(window.has_more())
    ))
}

fn render_structural_payload(
    payload: &SearchPlaneStructuralQueryResponse,
    rendered: &mut String,
) -> CliResult<()> {
    fmt_ok(writeln!(rendered, "kind: structural"))?;
    render_generation(&payload.generation, rendered)?;
    fmt_ok(writeln!(rendered, "results: {}", payload.results.len()))?;
    render_keyset_page_line(&payload.window, payload.examined, rendered)?;
    render_read_epoch_line(payload.read_epoch, rendered)?;
    render_continuation_cursor(payload.next_cursor.as_ref(), rendered)?;
    for (index, candidate) in payload.results.iter().enumerate() {
        let display_index = index
            .checked_add(1)
            .ok_or_else(|| CliError::protocol("structural candidate index overflow".to_string()))?;
        fmt_ok(writeln!(
            rendered,
            "{}. candidate_id={} bindings={}",
            display_index,
            candidate.candidate_id,
            candidate.bindings.len()
        ))?;
        for binding in &candidate.bindings {
            fmt_ok(writeln!(
                rendered,
                "   {}: bytes={}-{} lines={}-{}",
                binding.metavariable,
                binding.start_byte,
                binding.end_byte,
                binding.start_line,
                binding.end_line
            ))?;
        }
    }
    Ok(())
}

fn render_lexical_payload(
    kind: &str,
    payload: &TextQueryResponse,
    explanation: Option<&SearchExplanation>,
    rendered: &mut String,
) -> CliResult<()> {
    fmt_ok(writeln!(rendered, "kind: {kind}"))?;
    render_generation(&payload.generation, rendered)?;
    fmt_ok(writeln!(
        rendered,
        "rank_unit: {}",
        payload.rank_unit.as_str()
    ))?;
    fmt_ok(writeln!(rendered, "results: {}", payload.results.len()))?;
    for (index, candidate) in payload.results.iter().enumerate() {
        let display_index = index
            .checked_add(1)
            .ok_or_else(|| CliError::protocol("candidate index overflow".to_string()))?;
        fmt_ok(writeln!(
            rendered,
            "{}. candidate_id={} path={} lines={}-{} score={}",
            display_index,
            candidate.candidate_id,
            candidate.repo_relative_path.as_str(),
            candidate.start_line,
            candidate.end_line,
            candidate.score
        ))?;
        for line in candidate.snippet.lines() {
            fmt_ok(writeln!(rendered, "   {line}"))?;
        }
    }
    if let Some(file_owner_rows) = &payload.file_owner_rows {
        fmt_ok(writeln!(
            rendered,
            "file_owner_rows: {}",
            file_owner_rows.len()
        ))?;
        for (index, row) in file_owner_rows.iter().enumerate() {
            let display_index = index.checked_add(1).ok_or_else(|| {
                CliError::protocol("file owner projection index overflow".to_string())
            })?;
            let owners = if row.owners.is_empty() {
                "-".to_string()
            } else {
                row.owners.join(",")
            };
            fmt_ok(writeln!(
                rendered,
                "owner_row {}. candidate_id={} path={} owners={}",
                display_index,
                row.candidate_id,
                row.repo_relative_path.as_str(),
                owners
            ))?;
        }
    }
    render_continuation_cursor(payload.next_cursor.as_ref(), rendered)?;
    if let Some(explanation) = explanation {
        render_explanation(explanation, rendered)?;
    }
    Ok(())
}

fn render_symbol_payload(payload: &SymbolQueryResponse, rendered: &mut String) -> CliResult<()> {
    fmt_ok(writeln!(rendered, "kind: symbol"))?;
    render_generation(&payload.generation, rendered)?;
    fmt_ok(writeln!(rendered, "results: {}", payload.results.len()))?;
    for (index, candidate) in payload.results.iter().enumerate() {
        let display_index = index
            .checked_add(1)
            .ok_or_else(|| CliError::protocol("symbol candidate index overflow".to_string()))?;
        render_symbol_candidate(display_index, candidate, rendered)?;
    }
    render_continuation_cursor(payload.next_cursor.as_ref(), rendered)
}

/// The continuation of a ranked page, as `--cursor-json` takes it back.
fn render_continuation_cursor(
    cursor: Option<&ContinuationTokenV2>,
    rendered: &mut String,
) -> CliResult<()> {
    if let Some(cursor) = cursor {
        let json = serde_json::to_string(cursor)
            .map_err(|err| CliError::protocol(format!("encode next_cursor: {err}")))?;
        fmt_ok(writeln!(rendered, "next_cursor: {json}"))?;
    }
    Ok(())
}

fn render_symbol_candidate(
    display_index: usize,
    candidate: &SymbolCandidate,
    rendered: &mut String,
) -> CliResult<()> {
    let family = candidate.symbol_kind_family.map_or(
        "-",
        quanta_index_contract::lex::SymbolKindFamily::as_code_str,
    );
    fmt_ok(writeln!(
        rendered,
        "{}. candidate_id={} path={} lines={}-{} score={} symbol_kind={} symbol_kind_family={}",
        display_index,
        candidate.candidate_id,
        candidate.repo_relative_path.as_str(),
        candidate.start_line,
        candidate.end_line,
        candidate.score,
        candidate.symbol_kind.as_str(),
        family,
    ))?;
    for line in candidate.snippet.lines() {
        fmt_ok(writeln!(rendered, "   {line}"))?;
    }
    Ok(())
}

fn render_runtime_metadata_payload(
    payload: &SearchPlaneRuntimeMetadataQueryResponse,
    rendered: &mut String,
) -> CliResult<()> {
    fmt_ok(writeln!(rendered, "kind: runtime-metadata"))?;
    render_generation(&payload.generation, rendered)?;
    fmt_ok(writeln!(rendered, "results: {}", payload.results.len()))?;
    render_keyset_page_line(&payload.window, payload.examined, rendered)?;
    render_read_epoch_line(payload.read_epoch, rendered)?;
    fmt_ok(writeln!(
        rendered,
        "universe_epoch: {}",
        payload.universe_epoch
    ))?;
    render_continuation_cursor(payload.next_cursor.as_ref(), rendered)?;
    for (index, candidate) in payload.results.iter().enumerate() {
        let display_index = index.checked_add(1).ok_or_else(|| {
            CliError::protocol("runtime-metadata candidate index overflow".to_string())
        })?;
        fmt_ok(writeln!(
            rendered,
            "{}. candidate_id={} path={} lines={}-{} score={}",
            display_index,
            candidate.candidate_id,
            candidate.repo_relative_path.as_str(),
            candidate.start_line,
            candidate.end_line,
            candidate.score
        ))?;
        for line in candidate.snippet.lines() {
            fmt_ok(writeln!(rendered, "   {line}"))?;
        }
    }
    Ok(())
}

fn render_generation(generation: &GenerationPin, rendered: &mut String) -> CliResult<()> {
    fmt_ok(writeln!(
        rendered,
        "generation: repo_id={} revision_id={} manifest_generation={}",
        generation.repo_id.as_str(),
        generation.revision_id.as_str(),
        generation.manifest_generation.get()
    ))
}

fn render_explanation(explanation: &SearchExplanation, rendered: &mut String) -> CliResult<()> {
    fmt_ok(writeln!(rendered, "summary: {}", explanation.summary))?;
    fmt_ok(writeln!(rendered, "strategy: {}", explanation.strategy))?;
    let touched = explanation
        .engines_touched
        .iter()
        .map(engine_name)
        .collect::<Vec<_>>()
        .join(",");
    fmt_ok(writeln!(rendered, "engines_touched: {touched}"))?;
    if let Some(reason) = explanation.early_stop_reason {
        fmt_ok(writeln!(
            rendered,
            "early_stop_reason: {}",
            early_stop_name(reason)
        ))?;
    }
    fmt_ok(writeln!(
        rendered,
        "ranker_weights_hash: {}",
        encode_hex(&explanation.ranker_weights_hash)?
    ))?;
    if !explanation.planner_trace.is_empty() {
        fmt_ok(writeln!(rendered, "planner_trace:"))?;
        for PlannerTraceEntry { stage, detail } in &explanation.planner_trace {
            fmt_ok(writeln!(
                rendered,
                "  - {}: {}",
                planner_stage_name(*stage),
                detail
            ))?;
        }
    }
    if !explanation.contributions.is_empty() {
        fmt_ok(writeln!(rendered, "contributions:"))?;
        for contribution in &explanation.contributions {
            fmt_ok(writeln!(
                rendered,
                "  - {} signal_value={} weight={} contribution={}",
                contribution.signal_name,
                contribution.signal_value,
                contribution.weight,
                contribution.contribution
            ))?;
        }
    }
    Ok(())
}

fn response_kind_name(response: &SearchPlaneQueryIpcResponse) -> &'static str {
    match response {
        SearchPlaneQueryIpcResponse::ActiveGenerationSnapshot(_) => "ActiveGenerationSnapshot",
        SearchPlaneQueryIpcResponse::ResolvedLexicalGeneration(_) => "ResolvedLexicalGeneration",
        SearchPlaneQueryIpcResponse::Text(_) => "Text",
        SearchPlaneQueryIpcResponse::Symbol(_) => "Symbol",
        SearchPlaneQueryIpcResponse::Semantic(_) => "Semantic",
        SearchPlaneQueryIpcResponse::SemanticWorkBoundedV1(_) => "SemanticWorkBoundedV1",
        SearchPlaneQueryIpcResponse::Hybrid(_) => "Hybrid",
        SearchPlaneQueryIpcResponse::HybridSeed(_) => "HybridSeed",
        SearchPlaneQueryIpcResponse::History(_) => "History",
        SearchPlaneQueryIpcResponse::Structural(_) => "Structural",
        SearchPlaneQueryIpcResponse::RepoMapQuery(_) => "RepoMapQuery",
        SearchPlaneQueryIpcResponse::Explain(_) => "Explain",
        SearchPlaneQueryIpcResponse::Error(_) => "Error",
        SearchPlaneQueryIpcResponse::RuntimeMetadata(_) => "RuntimeMetadata",
        SearchPlaneQueryIpcResponse::ClusterMembershipRead(_) => "ClusterMembershipRead",
    }
}

fn command_kind_name(kind: CommandKind) -> &'static str {
    match kind {
        CommandKind::Lexical => "lexical",
        CommandKind::Symbol => "symbol",
        CommandKind::Semantic => "semantic",
        CommandKind::Hybrid => "hybrid",
        CommandKind::HybridSeed => "hybrid-seed",
        CommandKind::Explain => "explain",
        CommandKind::RepoMap => "repomap",
        CommandKind::RuntimeMetadata => "runtime-metadata",
        CommandKind::History => "history",
        CommandKind::Structural => "structural",
        CommandKind::Readiness => "readiness",
        CommandKind::RequestEvents => "events",
        CommandKind::GenerationStatus => "generation-status",
        CommandKind::Doctor => "doctor",
        CommandKind::Metrics => "metrics",
        CommandKind::Quarantine => "quarantine",
    }
}

fn planner_stage_name(stage: quanta_index_contract::PlannerStage) -> &'static str {
    stage.as_str()
}

fn engine_name(engine: &EngineTouched) -> &'static str {
    engine.as_str()
}

fn early_stop_name(reason: EarlyStopReason) -> &'static str {
    reason.as_str()
}

fn encode_hex(bytes: &[u8]) -> CliResult<String> {
    let mut out = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        out.push(hex_digit(*byte >> 4)?);
        out.push(hex_digit(*byte & 0x0f)?);
    }
    Ok(out)
}

fn hex_digit(nibble: u8) -> CliResult<char> {
    match nibble {
        0 => Ok('0'),
        1 => Ok('1'),
        2 => Ok('2'),
        3 => Ok('3'),
        4 => Ok('4'),
        5 => Ok('5'),
        6 => Ok('6'),
        7 => Ok('7'),
        8 => Ok('8'),
        9 => Ok('9'),
        10 => Ok('a'),
        11 => Ok('b'),
        12 => Ok('c'),
        13 => Ok('d'),
        14 => Ok('e'),
        15 => Ok('f'),
        _ => Err(CliError::protocol(format!(
            "hex nibble out of range: {nibble}"
        ))),
    }
}

fn fmt_ok(result: std::fmt::Result) -> CliResult<()> {
    result.map_err(|_err| CliError::protocol("string formatting failed".to_string()))
}

/// Keep the original SDK error rendering and exit classification while making
/// the committed publication available for reconciliation after a later error.
pub(super) fn render_after_publish_error_text(
    stage: PublishedBatchFailureStage,
    evidence: &PublishedBatchEvidence,
    source_text: &str,
) -> String {
    format!(
        "{source_text}\n  after publication stage: {stage:?}\n  publication: {:?}\n  receipt: {:?}",
        evidence.publication, evidence.receipt
    )
}

/// Render a typed remote error for the pretty CLI path (J7Q-06).
///
/// `code: message` stays the headline; when the wire carried typed repair
/// metadata it is appended as a distinct, scriptable hint block (class, the
/// supported alternative shapes, and the docs anchor). One renderer serves both
/// the wire-error and the `SdkError::Remote` arms so the guidance shape cannot
/// drift between them. The JSON path needs no special handling — it serializes
/// the whole envelope, `repair` included. This only renders guidance; it never
/// rewrites the query or softens the failure.
pub(super) fn render_remote_error_text(
    code: &quanta_index_contract::SearchPlaneErrorCodeV2,
    message: &str,
    repair: Option<&QueryErrorRepair>,
) -> CliResult<String> {
    let mut text = String::new();
    fmt_ok(write!(text, "{code}: {message}"))?;
    if let Some(repair) = repair {
        fmt_ok(write!(
            text,
            "\n  repair class: {}",
            repair.class.as_code_str()
        ))?;
        if !repair.supported_alternatives.is_empty() {
            fmt_ok(write!(
                text,
                "\n  try: {}",
                repair.supported_alternatives.join(" | ")
            ))?;
        }
        if let Some(anchor) = &repair.docs_anchor {
            fmt_ok(write!(text, "\n  docs: {anchor}"))?;
        }
    }
    Ok(text)
}
