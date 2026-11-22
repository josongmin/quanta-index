//! Operator-ergonomics rail (J7Q-05): snapshot the read-only diagnosis surfaces.
//!
//! This rail proves that the questions an operator routinely asks — "which
//! engines served this?", "what generation is active?", "what *typed* error did
//! the remote return?", "are perf/tail metrics queryable?" — are answerable from
//! **machine-readable** surfaces, with route / generation / typed-error
//! provenance preserved rather than swallowed behind human-only strings.
//!
//! It is a read-only first increment (per the ticket: read-only diagnosis before
//! any mutating workflow). It seeds a document in a pinned Legacy daemon via
//! the public SDK and captures four surfaces into `cli_snapshots.json`:
//!
//! - `result_provenance` — a served query's `engines_touched` labels plus the
//!   repo / revision / serving-generation stamp the candidate carries;
//! - `typed_error` — a rejected (empty) query's typed error code, asserting the
//!   typed failure is surfaced verbatim, never collapsed to a generic string;
//! - `perf_metrics` — the registered metric names from the obs snapshot, the
//!   surface an operator reads for tail/latency diagnosis;
//! - `generation_state` — the runtime's current generation plus its pin.
//!
//! Fail-closed posture: a surface that swallows its provenance (a generation
//! mismatch, the wrong typed refusal, or no query metrics) is a rail
//! failure recorded as `provenance_ok=false`; the rail does NOT fabricate or
//! guess missing runtime state.

use std::path::Path;

use anyhow::Result as AnyResult;
use quanta_index_contract::{
    MetricsSnapshotV1, SearchCorpusActiveHeadV1, SearchPlaneErrorCodeV2, TextQueryResponse,
};
use quanta_index_sdk::{GenerationPin, QuantaIndex, RepoId, RevisionId, SdkError};
use serde_json::{Value, json};

use crate::artifact::{
    BenchArtifactV1, BenchMode, BenchProvenanceV1, BenchRowV1, BenchSyntax, GitHeadV1, HostV1,
    PhaseDurationsV1, ResourceUsageV1, ResultShape, RouteFamily, config_digest, corpus_digest,
};
#[cfg(any(target_os = "macos", test))]
use crate::legacy_reference::peak_rss_from_darwin_metrics;
use crate::legacy_reference::{LegacySearchdBinary, publish_legacy_source, request_when_ready};
use crate::private_tempdir;

/// Repo id the ops fixture is ingested under.
const OPS_REPO: &str = "repo-e2e";
const OPS_PATH: &str = "src/ops.rs";
const OPS_SOURCE: &str =
    "fn ops_probe() {\n    // ops_diagnosis_needle marker for the operator rail\n}\n";
const BACKEND: &str = "legacy-reference";

#[cfg(target_os = "linux")]
const RSS_SOURCE: &str = "daemon-proc-vmhwm";
#[cfg(target_os = "macos")]
const RSS_SOURCE: &str = "daemon-ru-maxrss-gauge";

/// Result cap for the served diagnosis query.
const OPS_TOP_K: u32 = 10;

/// Token planted in the fixture so the served query returns a hit.
const OPS_QUERY: &str = "ops_diagnosis_needle";

/// One captured operator-diagnosis surface.
#[derive(Clone, Debug)]
pub struct OpsSnapshot {
    /// Stable surface id (artifact key + regression anchor).
    pub surface: &'static str,
    /// The operational question this surface answers, in plain words.
    pub operator_question: &'static str,
    /// The machine-readable snapshot bytes (stable field names).
    pub json: Value,
    /// Whether the surface preserved its provenance / typed signal. `false` is a
    /// blocking rail failure (a swallowed provenance, not a benign zero).
    pub provenance_ok: bool,
    /// Human-facing note recorded alongside the verdict.
    pub note: String,
}

/// The full ops report.
#[derive(Clone, Debug)]
pub struct OpsReport {
    pub snapshots: Vec<OpsSnapshot>,
    pub passed: bool,
    daemon_sha256: String,
    resources: ResourceUsageV1,
}

fn lexical_probe(
    client: &QuantaIndex,
    pin: &GenerationPin,
    text: &str,
) -> Result<TextQueryResponse, SdkError> {
    request_when_ready(|| {
        client
            .lexical()
            .query()
            .native(text)
            .pinned(pin.clone())
            .top_k(OPS_TOP_K)
            .execute()
    })
}

/// Capture the result-provenance surface from a served query.
///
/// The operator question is "where did this result come from?" — a served
/// candidate must carry its own repo / revision / serving-generation stamp so an
/// operator can attribute it without reading runtime internals. `engines_touched`
/// is recorded as-is (the single-engine lexical path leaves it implicit; it is
/// the explain surface that enumerates engines), so it is informational here, not
/// the gating signal.
fn capture_result_provenance(client: &QuantaIndex, pin: &GenerationPin) -> OpsSnapshot {
    let response = lexical_probe(client, pin, OPS_QUERY);
    let result = response.as_ref().ok();
    let candidate = result.and_then(|result| result.results.first());
    let engines: Vec<_> = result
        .into_iter()
        .flat_map(|result| &result.explanation.engines_touched)
        .map(|engine| format!("{engine:?}"))
        .collect();
    let first_provenance = candidate.map(|candidate| {
        json!({
            "repo_id": candidate.repo_id.as_str(),
            "revision_id": candidate.revision_id.as_str(),
            "serving_generation": format!("{:?}", candidate.manifest_generation),
            "repo_relative_path": candidate.repo_relative_path.as_str(),
            "score": candidate.score,
        })
    });
    let provenance_ok = result.is_some_and(|result| result.generation == *pin)
        && candidate.is_some_and(|candidate| {
            candidate.repo_id == pin.repo_id
                && candidate.revision_id == pin.revision_id
                && candidate.manifest_generation == pin.manifest_generation
                && candidate.repo_relative_path.as_str() == OPS_PATH
        });
    OpsSnapshot {
        surface: "result_provenance",
        operator_question: "where did a served result come from (repo / revision / serving generation)?",
        json: json!({
            "engines_touched": engines,
            "candidate_count": result.map_or(0, |result| result.results.len()),
            "first_candidate_provenance": first_provenance,
            "query_error": response.err().map(|error| error.to_string()),
        }),
        provenance_ok,
        note: if provenance_ok {
            "served result carries repo/revision/generation provenance".to_string()
        } else {
            "served result missing source provenance".to_string()
        },
    }
}

/// Capture the typed-error surface from a rejected (empty) query.
fn capture_typed_error(client: &QuantaIndex, pin: &GenerationPin) -> OpsSnapshot {
    let result = lexical_probe(client, pin, "");
    let (code, message, query_error) = match result {
        Err(SdkError::Remote { code, message, .. }) => (Some(code), Some(message), None),
        Err(error) => (None, None, Some(error.to_string())),
        Ok(_) => (None, None, None),
    };
    let provenance_ok = code == Some(SearchPlaneErrorCodeV2::InvalidRequest);
    OpsSnapshot {
        surface: "typed_error",
        operator_question: "what typed error does the remote return for a rejected query?",
        json: json!({
            "probe": "empty query text (policy rejection)",
            "typed_error_code": code.map(SearchPlaneErrorCodeV2::as_wire_str),
            "typed_error_message": message,
            "query_error": query_error,
        }),
        provenance_ok,
        note: if provenance_ok {
            "typed error code surfaced verbatim".to_string()
        } else {
            "empty query did not surface the daemon's INVALID_REQUEST refusal".to_string()
        },
    }
}

/// Capture the perf/observability surface (the tail-diagnosis read surface).
fn capture_perf_metrics(metrics: &Result<MetricsSnapshotV1, SdkError>) -> OpsSnapshot {
    let (names, count, served_count, latency_count, error) = match metrics {
        Ok(snapshot) => {
            let mut names = snapshot
                .counters
                .iter()
                .map(|counter| counter.name.as_str())
                .chain(
                    snapshot
                        .histograms
                        .iter()
                        .map(|histogram| histogram.name.as_str()),
                )
                .filter(|name| name.starts_with("lq_"))
                .collect::<Vec<_>>();
            names.sort_unstable();
            names.dedup();
            let latency_count = snapshot
                .histograms
                .iter()
                .find(|histogram| histogram.name == "lq_route_lexical_latency_ms")
                .map_or(0, |histogram| histogram.count);
            let served_count = snapshot
                .counters
                .iter()
                .find(|counter| counter.name == "lq_route_lexical_served_total")
                .map_or(0, |counter| counter.value);
            (
                names,
                snapshot.diagnostics.samples_recorded,
                served_count,
                latency_count,
                None,
            )
        }
        Err(err) => (Vec::new(), 0, 0, 0, Some(err.to_string())),
    };
    let provenance_ok = count > 0 && served_count > 0 && latency_count >= served_count;
    OpsSnapshot {
        surface: "perf_metrics",
        operator_question: "are perf / tail metrics queryable for diagnosis?",
        json: json!({
            "metric_sample_count": count,
            "metric_names": names,
            "lexical_served_count": served_count,
            "lexical_latency_observation_count": latency_count,
            "snapshot_error": error,
        }),
        provenance_ok,
        note: if provenance_ok {
            "obs metrics queryable".to_string()
        } else {
            "obs metric surface empty or unavailable".to_string()
        },
    }
}

/// Capture the generation-state surface (serving-head provenance).
fn capture_generation_state(
    client: &QuantaIndex,
    pin: &GenerationPin,
    expected: &SearchCorpusActiveHeadV1,
) -> OpsSnapshot {
    let publication = client
        .generations()
        .active_head(pin.repo_id.clone(), pin.revision_id.clone());
    let active = publication.as_ref().ok().and_then(Option::as_ref);
    let provenance_ok = active == Some(expected);
    OpsSnapshot {
        surface: "generation_state",
        operator_question: "what is the runtime's current generation and pin?",
        json: json!({
            "repo_id": pin.repo_id.as_str(),
            "revision_id": pin.revision_id.as_str(),
            "current_generation": active.map(|head| format!("{:?}", head.generation.lexical.manifest_generation)),
            "generation_pin": format!("{pin:?}"),
            "active_head_error": publication.err().map(|error| error.to_string()),
        }),
        provenance_ok,
        note: if provenance_ok {
            "daemon active head matches the fixture publication".to_string()
        } else {
            "daemon active head differs from the fixture publication or is unavailable".to_string()
        },
    }
}

/// Run the full ops rail against the selected pinned Legacy daemon.
pub fn run_ops_report() -> AnyResult<OpsReport> {
    let root = private_tempdir()?;
    let binary = LegacySearchdBinary::from_environment()
        .map_err(|error| anyhow::anyhow!("old-Q ops input: {error}"))?;
    let process = binary
        .start(root.path(), 8)
        .map_err(|error| anyhow::anyhow!("start old-Q ops process: {error}"))?;
    let client = process
        .connect()
        .map_err(|error| anyhow::anyhow!("connect old-Q ops process: {error}"))?;
    let repo = RepoId::new(OPS_REPO)?;
    let revision = RevisionId::new("rev-e2e")?;
    let (head, _receipt) = publish_legacy_source(
        &client,
        &repo,
        &revision,
        1,
        None,
        &[(OPS_PATH, OPS_SOURCE, 1)],
        None,
    )
    .map_err(|error| anyhow::anyhow!("publish old-Q ops fixture: {error}"))?;
    let pin = GenerationPin::new(repo, revision, head.generation.lexical.manifest_generation);
    let result = capture_result_provenance(&client, &pin);
    let typed_error = capture_typed_error(&client, &pin);
    let generation = capture_generation_state(&client, &pin, &head);
    let metrics = client.observability().metrics_snapshot();
    let snapshots = vec![
        result,
        typed_error,
        capture_perf_metrics(&metrics),
        generation,
    ];
    #[cfg(target_os = "linux")]
    let peak_rss_bytes = process.peak_rss_bytes()?;
    #[cfg(target_os = "macos")]
    let peak_rss_bytes = peak_rss_from_darwin_metrics(
        metrics
            .as_ref()
            .map_err(|error| anyhow::anyhow!("read old-Q ops metrics for peak RSS: {error}"))?,
    )?;
    drop(client);
    process
        .stop()
        .map_err(|error| anyhow::anyhow!("stop old-Q ops process: {error}"))?;
    binary
        .verify()
        .map_err(|error| anyhow::anyhow!("verify old-Q ops input: {error}"))?;
    let passed = snapshots.iter().all(|snapshot| snapshot.provenance_ok);
    Ok(OpsReport {
        snapshots,
        passed,
        daemon_sha256: binary.sha256().to_owned(),
        resources: ResourceUsageV1 { peak_rss_bytes },
    })
}

// ---------------------------------------------------------------------------
// Artifact emission.
// ---------------------------------------------------------------------------

fn snapshot_json(snapshot: &OpsSnapshot) -> Value {
    json!({
        "surface": snapshot.surface,
        "operator_question": snapshot.operator_question,
        "snapshot": snapshot.json,
        "provenance_ok": snapshot.provenance_ok,
        "note": snapshot.note,
    })
}

/// The CLI-snapshot supplement: not an authority artifact.
fn cli_snapshots_supplement_json(report: &OpsReport, git_head: &GitHeadV1) -> Value {
    json!({
        "kind": "quanta-index-benchmark-supplement",
        "supplement_schema_version": 1,
        "dimension": "ops",
        "git_head": git_head.as_str(),
        "backend": BACKEND,
        "daemon_sha256": report.daemon_sha256,
        "surfaces": report.snapshots.iter().map(snapshot_json).collect::<Vec<_>>(),
        "snapshot_note": "read-only operator-diagnosis surfaces captured machine-readably; route/generation/typed-error provenance is asserted preserved, never collapsed to a human-only string",
    })
}

/// The benchmark envelope binds the producer source and separately binds the Legacy daemon.
pub fn artifact(
    report: &OpsReport,
    git_head: GitHeadV1,
    host: HostV1,
) -> AnyResult<BenchArtifactV1> {
    let corpus = vec![(OPS_PATH.to_owned(), OPS_SOURCE.to_owned())];
    Ok(BenchArtifactV1 {
        dimension: "ops".to_string(),
        mode: BenchMode::Warm,
        concurrency: 1,
        provenance: BenchProvenanceV1 {
            git_head,
            corpus_digest: corpus_digest("ops", &corpus),
            config_digest: config_digest(
                "ops",
                &[
                    ("backend", BACKEND.to_owned()),
                    ("daemon_sha256", report.daemon_sha256.clone()),
                    ("rss_source", RSS_SOURCE.to_owned()),
                    ("surfaces", report.snapshots.len().to_string()),
                ],
            ),
            model_revision: None,
        },
        host,
        resources: report.resources,
        phases: PhaseDurationsV1::default(),
        disk_amplification: None,
        rows: report
            .snapshots
            .iter()
            .map(|snapshot| BenchRowV1 {
                scenario_id: format!("ops.{}", snapshot.surface),
                route_family: RouteFamily::RuntimeCatalog,
                syntax: BenchSyntax::Native,
                result_shape: ResultShape::Candidates,
                latency: None,
                qps: None,
                error_count: u64::from(!snapshot.provenance_ok),
                timeout_count: 0,
                result_count: Some(1),
                typed_error_code: None,
                engine_touched: vec!["operator_surface".to_string()],
                early_stop_reason: None,
            })
            .collect(),
        detail: json!({
            "passed": report.passed,
            "backend": BACKEND,
            "daemon_sha256": report.daemon_sha256,
            "resource_scope": "daemon",
            "rss_source": RSS_SOURCE,
            "blocking_signal": "every captured diagnosis surface must preserve its provenance (candidate identity, active head, typed refusal and query metrics)",
            "surfaces": report.snapshots.iter().map(snapshot_json).collect::<Vec<_>>(),
        }),
    })
}

/// Write the two canonical ops artifacts under `dir`:
/// `summary.json` and `cli_snapshots.json`.
pub fn write_artifacts(
    report: &OpsReport,
    dir: &Path,
    git_head: GitHeadV1,
    host: HostV1,
) -> AnyResult<()> {
    let snapshots = cli_snapshots_supplement_json(report, &git_head);
    artifact(report, git_head, host)?.write_to(&dir.join("summary.json"))?;
    crate::artifact::write_json_pretty(&dir.join("cli_snapshots.json"), &snapshots)?;
    Ok(())
}

#[cfg(test)]
#[expect(
    clippy::indexing_slicing,
    reason = "tests index JSON values whose shape this module constructs and asserts directly; an out-of-range index is a legitimate test failure"
)]
mod tests {
    use super::*;

    #[test]
    fn seeded_ops_diagnosis_uses_the_pinned_daemon() -> AnyResult<()> {
        let report = run_ops_report()?;
        anyhow::ensure!(report.snapshots.len() == 4, "wrong operator surface count");
        anyhow::ensure!(
            report.passed,
            "failed operator observations: {:?}",
            report.snapshots
        );
        anyhow::ensure!(
            report.resources.peak_rss_bytes > 0,
            "missing daemon peak RSS"
        );
        Ok(())
    }

    #[test]
    fn snapshot_json_preserves_surface_and_provenance() {
        let snapshot = OpsSnapshot {
            surface: "route_provenance",
            operator_question: "which engines?",
            json: json!({ "engines_touched": ["Lexical"] }),
            provenance_ok: true,
            note: "ok".to_string(),
        };
        let value = snapshot_json(&snapshot);
        assert_eq!(value["surface"], "route_provenance");
        assert_eq!(value["provenance_ok"], true);
        assert_eq!(value["snapshot"]["engines_touched"][0], "Lexical");
    }

    #[test]
    fn daemon_peak_gauge_requires_one_exact_positive_count() {
        use quanta_index_contract::MetricGaugeV1;

        let gauge = |value| MetricGaugeV1 {
            name: "process_resident_bytes".into(),
            value,
        };
        let mut report = MetricsSnapshotV1::default();
        assert!(peak_rss_from_darwin_metrics(&report).is_err());
        for value in [0.0, -1.0, 1.5, f64::NAN, 9_007_199_254_740_994.0] {
            report.gauges = vec![gauge(value)];
            assert!(peak_rss_from_darwin_metrics(&report).is_err(), "{value}");
        }
        report.gauges = vec![gauge(16_384.0)];
        assert_eq!(peak_rss_from_darwin_metrics(&report).ok(), Some(16_384));
        report.gauges.push(gauge(24_576.0));
        assert!(peak_rss_from_darwin_metrics(&report).is_err());
    }

    #[test]
    fn perf_metrics_require_served_query_observations() {
        use quanta_index_contract::{MetricCounterV1, MetricHistogramV1};

        let mut snapshot = MetricsSnapshotV1::default();
        snapshot.diagnostics.samples_recorded = 4;
        snapshot.histograms.push(MetricHistogramV1 {
            name: "lq_route_lexical_latency_ms".into(),
            count: 1,
            sum: 2.0,
            min: 2.0,
            max: 2.0,
            buckets: Vec::new(),
        });
        assert!(!capture_perf_metrics(&Ok(snapshot.clone())).provenance_ok);
        snapshot.counters.push(MetricCounterV1 {
            name: "lq_route_lexical_served_total".into(),
            value: 1,
        });
        assert!(capture_perf_metrics(&Ok(snapshot)).provenance_ok);
    }

    #[test]
    fn summary_passed_requires_all_surfaces_ok() {
        let ok = OpsSnapshot {
            surface: "a",
            operator_question: "?",
            json: json!({}),
            provenance_ok: true,
            note: String::new(),
        };
        let bad = OpsSnapshot {
            surface: "b",
            operator_question: "?",
            json: json!({}),
            provenance_ok: false,
            note: String::new(),
        };
        let pass_report = OpsReport {
            snapshots: vec![ok.clone()],
            passed: true,
            daemon_sha256: "c".repeat(64),
            resources: ResourceUsageV1 {
                peak_rss_bytes: 16_384,
            },
        };
        let fail_report = OpsReport {
            snapshots: vec![ok, bad],
            passed: false,
            ..pass_report.clone()
        };
        assert!(pass_report.passed);
        assert!(!fail_report.passed);
        let surfaces = cli_snapshots_supplement_json(
            &fail_report,
            &GitHeadV1::parse(&"a".repeat(40)).expect("head"),
        )["surfaces"]
            .as_array()
            .expect("surfaces array")
            .len();
        assert_eq!(surfaces, 2);
        let artifact = artifact(
            &pass_report,
            GitHeadV1::parse(&"a".repeat(40)).expect("head"),
            HostV1::observe().expect("host"),
        )
        .expect("authority artifact")
        .to_json()
        .expect("json");
        assert_eq!(artifact["schema_version"], 2);
        assert_eq!(artifact["detail"]["passed"], true);
        assert_eq!(artifact["detail"]["backend"], "legacy-reference");
        assert_eq!(artifact["detail"]["daemon_sha256"], "c".repeat(64));
        assert_eq!(artifact["detail"]["resource_scope"], "daemon");
        assert_eq!(artifact["resources"]["peak_rss_bytes"], 16_384);
    }
}
