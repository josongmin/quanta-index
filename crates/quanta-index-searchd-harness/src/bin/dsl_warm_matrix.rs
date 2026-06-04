//! `dsl_warm_matrix` — warm steady-state DSL latency artifact producer.
//!
//! Unlike the exploratory criterion bench, this binary is the benchmark gate
//! authority for warm mode. Each scenario is measured through multiple
//! dedicated warm passes; every pass boots a fresh runtime, primes the exact
//! scenario query untimed, then records a timed window. The final artifact row
//! is the median tail across those isolated passes, not a criterion byproduct.

use std::path::PathBuf;
use std::process::ExitCode;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result as AnyResult;
use quanta_index_searchd_harness::artifact::{BenchArtifact, BenchMode, BenchRow, LatencySummary};
use quanta_index_searchd_harness::bench_support::{
    QueryOutcome, prepare_cold_runtime, run_scenario_query,
};
use quanta_index_searchd_harness::scenarios::{DslBenchScenario, SCENARIOS};

const DEFAULT_WARM_SAMPLES: usize = 100;
const DEFAULT_WARM_REPEATS: usize = 5;
const DEFAULT_WARM_COOLDOWN_MS: u64 = 10;
const DEFAULT_PASS_SETTLE_MS: u64 = 5;
const DEFAULT_WARM_PRIME_QUERIES: usize = 5;

fn git_rev() -> String {
    if let Ok(rev) = std::env::var("DSL_BENCH_GIT_REV") {
        return rev;
    }
    "unknown".to_string()
}

fn parse_usize_env(name: &str, default: usize) -> usize {
    match std::env::var(name) {
        Ok(raw) => match raw.parse::<usize>() {
            Ok(n) if n > 0 => n,
            _ => default,
        },
        Err(_) => default,
    }
}

fn parse_u64_env(name: &str, default: u64) -> u64 {
    match std::env::var(name) {
        Ok(raw) => match raw.parse::<u64>() {
            Ok(n) => n,
            Err(_) => default,
        },
        Err(_) => default,
    }
}

fn warm_samples() -> usize {
    parse_usize_env("DSL_BENCH_WARM_SAMPLES", DEFAULT_WARM_SAMPLES)
}

fn warm_repeats() -> usize {
    parse_usize_env("DSL_BENCH_WARM_REPEATS", DEFAULT_WARM_REPEATS)
}

fn warm_prime_queries() -> usize {
    parse_usize_env("DSL_BENCH_WARM_PRIME_QUERIES", DEFAULT_WARM_PRIME_QUERIES)
}

fn warm_inter_sample_cooldown() -> Duration {
    Duration::from_millis(parse_u64_env(
        "DSL_BENCH_WARM_COOLDOWN_MS",
        DEFAULT_WARM_COOLDOWN_MS,
    ))
}

fn warm_inter_pass_settle() -> Duration {
    Duration::from_millis(parse_u64_env(
        "DSL_BENCH_WARM_PASS_SETTLE_MS",
        DEFAULT_PASS_SETTLE_MS,
    ))
}

fn parse_out_path() -> PathBuf {
    let mut args = std::env::args().skip(1);
    match (args.next().as_deref(), args.next()) {
        (Some("--out"), Some(path)) => PathBuf::from(path),
        _ => std::env::var_os("DSL_BENCH_WARM_OUT").map_or_else(
            || PathBuf::from("artifacts/dsl-bench/warm-matrix.json"),
            PathBuf::from,
        ),
    }
}

fn elapsed_ms(started: Instant) -> f64 {
    started.elapsed().as_secs_f64() * 1000.0
}

fn aggregate_samples(samples: &[f64], scenario_id: &str) -> AnyResult<LatencySummary> {
    LatencySummary::from_samples_ms(samples)
        .ok_or_else(|| anyhow::anyhow!("empty warm sample set for {scenario_id}"))
}

fn measure_pass(
    scenario: &DslBenchScenario,
    samples_n: usize,
    prime_queries: usize,
    cooldown: Duration,
    settle: Duration,
) -> AnyResult<(
    Vec<f64>,
    quanta_index_searchd_harness::bench_support::QueryOutcome,
)> {
    let mut runtime = prepare_cold_runtime(scenario)?;
    for _ in 0..prime_queries {
        let _warmup = run_scenario_query(&mut runtime, scenario);
        thread::sleep(settle);
    }
    let mut samples = Vec::with_capacity(samples_n);
    let mut last = run_scenario_query(&mut runtime, scenario);
    for _ in 0..samples_n {
        thread::sleep(cooldown);
        let started = Instant::now();
        last = run_scenario_query(&mut runtime, scenario);
        samples.push(elapsed_ms(started));
    }
    Ok((samples, last))
}

fn main() -> ExitCode {
    let out = parse_out_path();
    let rev = git_rev();
    let samples_n = warm_samples();
    let repeats = warm_repeats();
    let prime_queries = warm_prime_queries();
    let cooldown = warm_inter_sample_cooldown();
    let pass_settle = warm_inter_pass_settle();

    let mut artifact = BenchArtifact::new(BenchMode::Warm, rev);
    let mut pass_samples: Vec<Vec<f64>> = SCENARIOS
        .iter()
        .map(|_| Vec::with_capacity(samples_n.saturating_mul(repeats)))
        .collect();
    let mut last_outcomes: Vec<Option<QueryOutcome>> = SCENARIOS.iter().map(|_| None).collect();

    for _ in 0..repeats {
        for (scenario_idx, scenario) in SCENARIOS.iter().enumerate() {
            let (samples, outcome) =
                match measure_pass(scenario, samples_n, prime_queries, cooldown, pass_settle) {
                    Ok(result) => result,
                    Err(err) => {
                        eprintln!("dsl_warm_matrix: scenario {} failed: {err:#}", scenario.id);
                        return ExitCode::FAILURE;
                    }
                };
            pass_samples[scenario_idx].extend(samples);
            last_outcomes[scenario_idx] = Some(outcome);
        }
    }

    for (scenario_idx, scenario) in SCENARIOS.iter().enumerate() {
        let Some(last) = last_outcomes[scenario_idx].take() else {
            eprintln!("dsl_warm_matrix: no outcome for {}", scenario.id);
            return ExitCode::FAILURE;
        };
        artifact.rows.push(BenchRow {
            scenario_id: scenario.id.to_string(),
            route_family: scenario.route_family,
            syntax: scenario.syntax,
            mode: BenchMode::Warm,
            result_shape: last.result_shape,
            latency: match aggregate_samples(&pass_samples[scenario_idx], scenario.id) {
                Ok(summary) => Some(summary),
                Err(err) => {
                    eprintln!("dsl_warm_matrix: scenario {} failed: {err:#}", scenario.id);
                    return ExitCode::FAILURE;
                }
            },
            result_count: last.result_count,
            typed_error_code: last.typed_error_code,
            engine_touched: last.engine_touched,
            early_stop_reason: last.early_stop_reason,
        });
    }

    match artifact.write_to(&out) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("dsl_warm_matrix: failed to write {}: {err}", out.display());
            ExitCode::FAILURE
        }
    }
}
