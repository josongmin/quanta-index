//! Layer-3 warm steady-state DSL query-latency matrix (JUN-08-001).
//!
//! Boots the runtime **once**, ingests a deterministic fixture, seals +
//! activates, then for each shipped DSL scenario:
//!
//!   1. drives a criterion bench (the human-facing regression view), and
//!   2. collects a manual sample loop to emit a machine-readable artifact row
//!      with `p50/p95/p99` per scenario (criterion does not expose those
//!      percentiles programmatically).
//!
//! The artifact is one `BenchArtifactV1` (QI-BB-010): the exact head of a
//! clean worktree, the warm fixture corpus digest, the sample configuration
//! digest, the host and the process's peak RSS. A dirty tree or an
//! unresolvable head refuses to write. This is the exploratory criterion
//! view; the benchmark gate authority for warm mode is `dsl_warm_matrix`.
//!
//! Output path: `$DSL_BENCH_WARM_OUT` if set, else
//! `artifacts/dsl-bench/warm-matrix.json` relative to the process CWD.

use std::path::{Path, PathBuf};
use std::time::Instant;

use criterion::Criterion;

use quanta_index_searchd_harness::artifact::{
    BenchArtifactV1, BenchMode, BenchProvenanceV1, GitHeadV1, HostV1, LatencySummary,
    PhaseDurationsV1, ResourceUsageV1, config_digest, corpus_digest, model_revision_of,
};
use quanta_index_searchd_harness::bench_support::{
    ScenarioTruthMode, bench_row, prepare_warm_runtime, run_scenario_query,
    validate_scenario_outcome, warm_fixture_corpus_files,
};
use quanta_index_searchd_harness::scenarios::SCENARIOS;

const DIMENSION: &str = "dsl-warm";

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
        || PathBuf::from("artifacts/dsl-bench/warm-matrix.json"),
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
    let mut criterion = Criterion::default().configure_from_args();
    let mut runtime = prepare_warm_runtime()?;
    let model_revision = model_revision_of(runtime.embedder_profile());
    let mut rows = Vec::with_capacity(SCENARIOS.len());

    {
        let mut group = criterion.benchmark_group("dsl_query_matrix");
        for scenario in SCENARIOS {
            let probe = run_scenario_query(&mut runtime, scenario);
            validate_scenario_outcome(scenario, ScenarioTruthMode::SharedWarmFixture, &probe)?;
            let _registered: &mut criterion::BenchmarkGroup<'_, criterion::measurement::WallTime> =
                group.bench_function(scenario.id, |b| {
                    b.iter(|| {
                        let outcome = run_scenario_query(&mut runtime, scenario);
                        let _kept = criterion::black_box(&outcome);
                    });
                });

            let mut samples = Vec::with_capacity(samples_n);
            let mut last = run_scenario_query(&mut runtime, scenario);
            for _ in 0..samples_n {
                let started = Instant::now();
                last = run_scenario_query(&mut runtime, scenario);
                samples.push(elapsed_ms(started));
            }
            validate_scenario_outcome(scenario, ScenarioTruthMode::SharedWarmFixture, &last)?;
            rows.push(bench_row(
                scenario,
                last,
                LatencySummary::from_samples_ms(&samples),
            ));
        }
        group.finish();
    }

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
                    ("source", "criterion-shared-warm-fixture".to_string()),
                ],
            ),
            model_revision,
        },
        host,
        resources: ResourceUsageV1::observe_self()?,
        phases: PhaseDurationsV1::default(),
        disk_amplification: None,
        rows,
        detail: serde_json::json!({
            "source": "criterion-shared-warm-fixture",
            "samples": samples_n,
        }),
    };
    artifact.write_to(&warm_out_path())?;
    criterion.final_summary();
    drop(criterion);
    Ok(())
}
