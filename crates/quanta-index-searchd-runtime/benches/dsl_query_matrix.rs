//! Exploratory pinned-Legacy SDK warm DSL query-latency matrix (JUN-08-001).
//!
//! Boots one pinned old-Q child, publishes the deterministic full warm fixture
//! through the public SDK, and keeps that process for every shipped scenario:
//!
//!   1. drives a criterion bench (the human-facing regression view), and
//!   2. collects a manual sample loop to emit a machine-readable artifact row
//!      with `p50/p95/p99` per scenario (criterion does not expose those
//!      percentiles programmatically).
//!
//! The artifact is one `BenchArtifactV1` (QI-BB-010): the exact head of a
//! clean worktree, the warm fixture corpus digest, the sample configuration
//! digest (including the executable SHA), the host and the child daemon's
//! peak RSS. A dirty tree or an
//! unresolvable head refuses to write. This is the exploratory criterion
//! view; the benchmark gate authority for warm mode is `dsl_warm_matrix`.
//!
//! Output path: `$DSL_BENCH_WARM_OUT` if set, else
//! `artifacts/dsl-bench/criterion-pinned-legacy-sdk.json` relative to CWD.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use criterion::Criterion;

use quanta_index_searchd::app::SemanticEmbedderProfile;
use quanta_index_searchd_harness::artifact::{
    BenchArtifactV1, BenchMode, BenchProvenanceV1, GitHeadV1, HostV1, LatencySummary,
    PhaseDurationsV1, ResourceUsageV1, config_digest, corpus_digest, model_revision_of,
};
use quanta_index_searchd_harness::bench_support::{
    ScenarioTruthMode, bench_row, pinned_process, validate_scenario_outcome,
    warm_fixture_corpus_files,
};
use quanta_index_searchd_harness::legacy_reference::LegacySearchdBinary;
use quanta_index_searchd_harness::scenarios::SCENARIOS;

const DIMENSION: &str = "dsl-warm";
const SOURCE: &str = "criterion-pinned-legacy-sdk";
const TIMING_BOUNDARY: &str = "public-sdk-query-call-only; validation-excluded";

/// Manual percentile-sample count per scenario (in addition to the criterion
/// timing pass).
///
/// Overridable via `$DSL_BENCH_WARM_SAMPLES` for quick runs; an unparsable
/// or zero value is a refusal, not a silent default.
fn warm_samples() -> anyhow::Result<usize> {
    const DEFAULT: usize = 200;
    let Ok(raw) = std::env::var("DSL_BENCH_WARM_SAMPLES") else {
        return Ok(DEFAULT);
    };
    match raw.parse::<usize>() {
        Ok(n) if n > 0 => Ok(n),
        Ok(n) => Err(anyhow::anyhow!(
            "DSL_BENCH_WARM_SAMPLES={n}: must be at least 1"
        )),
        Err(err) => Err(anyhow::anyhow!(
            "DSL_BENCH_WARM_SAMPLES={raw:?}: not an integer: {err}"
        )),
    }
}

fn warm_out_path() -> PathBuf {
    std::env::var_os("DSL_BENCH_WARM_OUT").map_or_else(
        || PathBuf::from("artifacts/dsl-bench/criterion-pinned-legacy-sdk.json"),
        PathBuf::from,
    )
}

fn elapsed_ms(started: Instant) -> f64 {
    started.elapsed().as_secs_f64() * 1000.0
}

fn main() -> anyhow::Result<()> {
    // Provenance first: a run that cannot be attributed is not started.
    let git_head = GitHeadV1::resolve(Path::new("."))?;
    let host = HostV1::observe()?;
    let samples_n = warm_samples()?;
    let binary = LegacySearchdBinary::from_environment()
        .map_err(|error| anyhow::anyhow!("Criterion pinned Legacy executable: {error}"))?;
    let daemon_sha256 = binary.sha256().to_owned();
    let out = warm_out_path();
    let criterion = Criterion::default();
    // Registered capture supplies a fresh CRITERION_HOME. Direct invocations
    // must also avoid reusing the older in-process samples with the same IDs.
    let mut criterion = if std::env::var_os("CRITERION_HOME").is_some() {
        criterion
    } else {
        criterion.output_directory(&out.with_extension("criterion").join(&daemon_sha256))
    }
    .configure_from_args();
    let model_revision = model_revision_of(&SemanticEmbedderProfile::hash_dev());
    // The fixture is eager: Criterion --list and --test both require the
    // pinned executable and exercise the real SDK. Neither can silently
    // substitute the in-process runtime or an unmeasured fixture.
    let (rows, resources) = pinned_process::with_fixture(&binary, None, |client, pin, process| {
        let mut rows = Vec::with_capacity(SCENARIOS.len());
        let mut group = criterion.benchmark_group("dsl_query_matrix");
        for scenario in SCENARIOS {
            let probe = pinned_process::run_scenario_query(client, pin, scenario)?;
            validate_scenario_outcome(scenario, ScenarioTruthMode::SharedWarmFixture, &probe)?;
            let mut criterion_error = None;
            let _registered: &mut criterion::BenchmarkGroup<'_, criterion::measurement::WallTime> =
                group.bench_function(scenario.id, |b| {
                    b.iter_custom(|iterations| {
                        if criterion_error.is_some() {
                            // Criterion 0.5.1 rejects a zero sample before
                            // normalization or saving benchmark output. Its
                            // warm-up uses wall time, so avoid a busy poll.
                            std::thread::sleep(Duration::from_millis(10));
                            return Duration::ZERO;
                        }
                        let mut measured = Duration::ZERO;
                        for _ in 0..iterations {
                            let started = Instant::now();
                            let outcome =
                                match pinned_process::run_scenario_query(client, pin, scenario) {
                                    Ok(outcome) => outcome,
                                    Err(error) => {
                                        criterion_error = Some(anyhow::anyhow!(
                                            "Criterion SDK scenario {} failed: {error:#}",
                                            scenario.id
                                        ));
                                        break;
                                    }
                                };
                            let elapsed = started.elapsed();
                            if let Err(error) = validate_scenario_outcome(
                                scenario,
                                ScenarioTruthMode::SharedWarmFixture,
                                &outcome,
                            ) {
                                criterion_error = Some(anyhow::anyhow!(
                                    "Criterion SDK scenario {} mismatched: {error:#}",
                                    scenario.id
                                ));
                                break;
                            }
                            let Some(total) = measured.checked_add(elapsed) else {
                                criterion_error = Some(anyhow::anyhow!(
                                    "Criterion SDK scenario {} elapsed overflow",
                                    scenario.id
                                ));
                                break;
                            };
                            measured = total;
                            let _kept = criterion::black_box(outcome);
                        }
                        if criterion_error.is_some() {
                            Duration::ZERO
                        } else {
                            measured
                        }
                    });
                });
            if let Some(error) = criterion_error {
                return Err(error);
            }

            let mut samples = Vec::with_capacity(samples_n);
            let mut last = pinned_process::run_scenario_query(client, pin, scenario)?;
            validate_scenario_outcome(scenario, ScenarioTruthMode::SharedWarmFixture, &last)?;
            for _ in 0..samples_n {
                let started = Instant::now();
                last = pinned_process::run_scenario_query(client, pin, scenario)?;
                samples.push(elapsed_ms(started));
                validate_scenario_outcome(scenario, ScenarioTruthMode::SharedWarmFixture, &last)?;
            }
            let latency = LatencySummary::from_samples_ms(&samples)
                .ok_or_else(|| anyhow::anyhow!("scenario {} has no valid samples", scenario.id))?;
            rows.push(bench_row(scenario, last, Some(latency)));
        }
        group.finish();
        let peak_rss_bytes = process.observe_peak_rss(client)?;
        Ok((rows, ResourceUsageV1 { peak_rss_bytes }))
    })?;

    let artifact = BenchArtifactV1 {
        dimension: DIMENSION.to_string(),
        mode: BenchMode::Warm,
        concurrency: 1,
        provenance: BenchProvenanceV1 {
            git_head,
            corpus_digest: corpus_digest(DIMENSION, &warm_fixture_corpus_files()),
            config_digest: config_digest(
                DIMENSION,
                &[
                    ("samples", samples_n.to_string()),
                    ("scenario_count", SCENARIOS.len().to_string()),
                    ("source", SOURCE.to_string()),
                    ("daemon_sha256", daemon_sha256.clone()),
                    ("timing_boundary", TIMING_BOUNDARY.to_string()),
                    ("qualification", "diagnostic".to_string()),
                ],
            ),
            model_revision,
        },
        host,
        resources,
        phases: PhaseDurationsV1::default(),
        disk_amplification: None,
        rows,
        detail: serde_json::json!({
            "source": SOURCE,
            "qualification": "diagnostic",
            "daemon_sha256": daemon_sha256,
            "timing_boundary": TIMING_BOUNDARY,
            "rss_scope": "pinned_legacy_child_peak",
            "samples": samples_n,
        }),
    };
    artifact.write_to(&out)?;
    criterion.final_summary();
    drop(criterion);
    Ok(())
}
