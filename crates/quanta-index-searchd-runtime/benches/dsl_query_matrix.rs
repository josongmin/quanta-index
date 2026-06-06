//! Layer-3 warm steady-state DSL query-latency matrix (RFC-DSL-Benchmarking §3.1).
//!
//! Boots the runtime **once**, ingests a deterministic fixture, seals +
//! activates, then for each shipped DSL scenario:
//!
//!   1. drives a criterion bench (the human-facing regression view), and
//!   2. collects a manual sample loop to emit a machine-readable artifact row
//!      with `p50/p95/p99` per scenario (criterion does not expose those
//!      percentiles programmatically).
//!
//! v1 measures the **lexical** family. History / runtime-catalog / structural
//! scenarios are recorded with an explicit `early_stop_reason =
//! "fixture_not_seeded"` and a null latency — never a fabricated number.
//!
//! Output path: `$DSL_BENCH_WARM_OUT` if set, else
//! `artifacts/dsl-bench/warm-matrix.json` relative to the process CWD.
//! `$DSL_BENCH_GIT_REV` stamps the artifact's `git_rev`.

use std::path::PathBuf;
use std::time::Instant;

use criterion::Criterion;

use quanta_index_searchd_harness::artifact::{BenchArtifact, BenchMode, BenchRow, LatencySummary};
use quanta_index_searchd_harness::bench_support::{
    ScenarioTruthMode, prepare_warm_runtime, run_scenario_query, validate_scenario_outcome,
};
use quanta_index_searchd_harness::scenarios::SCENARIOS;

/// Manual percentile-sample count per scenario (in addition to the criterion
/// timing pass). Overridable via `$DSL_BENCH_WARM_SAMPLES` for quick runs.
fn warm_samples() -> usize {
    const DEFAULT: usize = 200;
    let Ok(raw) = std::env::var("DSL_BENCH_WARM_SAMPLES") else {
        return DEFAULT;
    };
    match raw.parse::<usize>() {
        Ok(n) if n > 0 => n,
        _ => DEFAULT,
    }
}

fn warm_out_path() -> PathBuf {
    std::env::var_os("DSL_BENCH_WARM_OUT").map_or_else(
        || PathBuf::from("artifacts/dsl-bench/warm-matrix.json"),
        PathBuf::from,
    )
}

fn git_rev() -> String {
    if let Ok(rev) = std::env::var("DSL_BENCH_GIT_REV") {
        return rev;
    }
    "unknown".to_string()
}

fn elapsed_ms(started: Instant) -> f64 {
    started.elapsed().as_secs_f64() * 1000.0
}

fn main() -> anyhow::Result<()> {
    let mut criterion = Criterion::default().configure_from_args();
    let mut runtime = prepare_warm_runtime()?;
    let mut artifact = BenchArtifact::new(BenchMode::Warm, git_rev());

    {
        let samples_n = warm_samples();
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

            artifact.rows.push(BenchRow {
                scenario_id: scenario.id.to_string(),
                route_family: scenario.route_family,
                syntax: scenario.syntax,
                mode: BenchMode::Warm,
                result_shape: last.result_shape,
                latency: LatencySummary::from_samples_ms(&samples),
                result_count: last.result_count,
                typed_error_code: last.typed_error_code,
                engine_touched: last.engine_touched,
                early_stop_reason: last.early_stop_reason,
            });
        }
        group.finish();
    }

    artifact.write_to(&warm_out_path())?;
    criterion.final_summary();
    drop(criterion);
    Ok(())
}
