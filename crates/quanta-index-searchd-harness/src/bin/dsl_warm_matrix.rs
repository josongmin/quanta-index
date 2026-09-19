//! `dsl_warm_matrix` — warm steady-state DSL latency artifact producer.
//!
//! Unlike the exploratory criterion bench, this binary is the benchmark gate
//! authority for warm mode. Each scenario is measured through multiple
//! dedicated warm passes; every pass boots a fresh runtime, primes the exact
//! scenario query untimed, then records a timed window. The final artifact row
//! is the median tail across those isolated passes, not a criterion byproduct.
//!
//! The artifact is one `BenchArtifactV1` (QI-BB-010): it names the exact
//! head of a clean worktree, the fixture corpus digest, the run
//! configuration digest, the host and the process's peak RSS. A dirty tree
//! or an unresolvable head is a refusal, never an `unknown` stamp.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result as AnyResult;
use quanta_index_searchd_harness::artifact::{
    BenchArtifactV1, BenchMode, BenchProvenanceV1, GitHeadV1, HostV1, LatencySummary,
    PhaseDurationsV1, ResourceUsageV1, config_digest, corpus_digest,
};
use quanta_index_searchd_harness::bench_support::{
    QueryOutcome, ScenarioTruthMode, bench_row, fixture_corpus_files, prepare_cold_runtime,
    run_scenario_query, validate_scenario_outcome,
};
use quanta_index_searchd_harness::scenarios::{DslBenchScenario, SCENARIOS};

const DIMENSION: &str = "dsl-warm";
const DEFAULT_WARM_SAMPLES: usize = 100;
const DEFAULT_WARM_REPEATS: usize = 5;
const DEFAULT_WARM_COOLDOWN_MS: u64 = 10;
const DEFAULT_PASS_SETTLE_MS: u64 = 5;
const DEFAULT_WARM_PRIME_QUERIES: usize = 5;

/// A run knob read from the environment; an unparsable or zero value is a
/// refusal, not a silent default.
fn parse_usize_env(name: &str, default: usize) -> AnyResult<usize> {
    let Ok(raw) = std::env::var(name) else {
        return Ok(default);
    };
    match raw.parse::<usize>() {
        Ok(n) if n > 0 => Ok(n),
        Ok(n) => Err(anyhow::anyhow!("{name}={n}: must be at least 1")),
        Err(err) => Err(anyhow::anyhow!("{name}={raw:?}: not an integer: {err}")),
    }
}

fn parse_u64_env(name: &str, default: u64) -> AnyResult<u64> {
    let Ok(raw) = std::env::var(name) else {
        return Ok(default);
    };
    raw.parse::<u64>()
        .map_err(|err| anyhow::anyhow!("{name}={raw:?}: not an integer: {err}"))
}

/// The run's knobs, read once and digested into the artifact's config
/// digest.
struct WarmConfig {
    samples: usize,
    repeats: usize,
    prime_queries: usize,
    cooldown: Duration,
    pass_settle: Duration,
}

impl WarmConfig {
    fn from_env() -> AnyResult<Self> {
        Ok(Self {
            samples: parse_usize_env("DSL_BENCH_WARM_SAMPLES", DEFAULT_WARM_SAMPLES)?,
            repeats: parse_usize_env("DSL_BENCH_WARM_REPEATS", DEFAULT_WARM_REPEATS)?,
            prime_queries: parse_usize_env(
                "DSL_BENCH_WARM_PRIME_QUERIES",
                DEFAULT_WARM_PRIME_QUERIES,
            )?,
            cooldown: Duration::from_millis(parse_u64_env(
                "DSL_BENCH_WARM_COOLDOWN_MS",
                DEFAULT_WARM_COOLDOWN_MS,
            )?),
            pass_settle: Duration::from_millis(parse_u64_env(
                "DSL_BENCH_WARM_PASS_SETTLE_MS",
                DEFAULT_PASS_SETTLE_MS,
            )?),
        })
    }

    fn digest(&self) -> String {
        config_digest(
            DIMENSION,
            &[
                ("samples", self.samples.to_string()),
                ("repeats", self.repeats.to_string()),
                ("prime_queries", self.prime_queries.to_string()),
                ("cooldown_ms", self.cooldown.as_millis().to_string()),
                ("pass_settle_ms", self.pass_settle.as_millis().to_string()),
                ("scenario_count", SCENARIOS.len().to_string()),
            ],
        )
    }
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
    config: &WarmConfig,
) -> AnyResult<(Vec<f64>, QueryOutcome, Option<String>)> {
    let mut runtime = prepare_cold_runtime(scenario)?;
    let model_revision =
        quanta_index_searchd_harness::artifact::model_revision_of(runtime.embedder_profile());
    for _ in 0..config.prime_queries {
        let warmup = run_scenario_query(&mut runtime, scenario);
        validate_scenario_outcome(scenario, ScenarioTruthMode::IsolatedFixture, &warmup)?;
        thread::sleep(config.pass_settle);
    }
    let mut samples = Vec::with_capacity(config.samples);
    let mut last = run_scenario_query(&mut runtime, scenario);
    for _ in 0..config.samples {
        thread::sleep(config.cooldown);
        let started = Instant::now();
        last = run_scenario_query(&mut runtime, scenario);
        samples.push(elapsed_ms(started));
    }
    validate_scenario_outcome(scenario, ScenarioTruthMode::IsolatedFixture, &last)?;
    Ok((samples, last, model_revision))
}

fn run(out: &Path) -> AnyResult<()> {
    // Provenance first: a run that cannot be attributed is not started.
    let git_head = GitHeadV1::resolve(Path::new("."))?;
    let host = HostV1::observe()?;
    let config = WarmConfig::from_env()?;

    let mut pass_samples: Vec<Vec<f64>> = SCENARIOS
        .iter()
        .map(|_| Vec::with_capacity(config.samples.saturating_mul(config.repeats)))
        .collect();
    let mut last_outcomes: Vec<Option<QueryOutcome>> = SCENARIOS.iter().map(|_| None).collect();
    let mut model_revision: Option<String> = None;

    for _ in 0..config.repeats {
        for (scenario_idx, scenario) in SCENARIOS.iter().enumerate() {
            let (samples, outcome, revision) = measure_pass(scenario, &config)
                .map_err(|err| anyhow::anyhow!("scenario {} failed: {err:#}", scenario.id))?;
            let Some(slot) = pass_samples.get_mut(scenario_idx) else {
                return Err(anyhow::anyhow!("no sample slot for {}", scenario.id));
            };
            slot.extend(samples);
            if let Some(outcome_slot) = last_outcomes.get_mut(scenario_idx) {
                *outcome_slot = Some(outcome);
            }
            model_revision = revision;
        }
    }

    let mut rows = Vec::with_capacity(SCENARIOS.len());
    for (scenario_idx, scenario) in SCENARIOS.iter().enumerate() {
        let Some(last) = last_outcomes.get_mut(scenario_idx).and_then(Option::take) else {
            return Err(anyhow::anyhow!("no outcome for {}", scenario.id));
        };
        let Some(samples) = pass_samples.get(scenario_idx) else {
            return Err(anyhow::anyhow!("no samples for {}", scenario.id));
        };
        let latency = aggregate_samples(samples, scenario.id)?;
        rows.push(bench_row(scenario, last, Some(latency)));
    }

    let artifact = BenchArtifactV1 {
        dimension: DIMENSION.to_string(),
        mode: BenchMode::Warm,
        concurrency: 1,
        provenance: BenchProvenanceV1 {
            git_head,
            // Every pass seeds one scenario's own fixture: the corpus is the
            // sequence of those fixtures, in scenario order.
            corpus_digest: corpus_digest(
                DIMENSION,
                &SCENARIOS
                    .iter()
                    .flat_map(|scenario| fixture_corpus_files(scenario.fixture))
                    .collect::<Vec<_>>(),
            ),
            config_digest: config.digest(),
            model_revision,
        },
        host,
        resources: ResourceUsageV1::observe_self()?,
        phases: PhaseDurationsV1::default(),
        disk_amplification: None,
        rows,
        detail: serde_json::json!({
            "samples_per_pass": config.samples,
            "repeats": config.repeats,
            "prime_queries": config.prime_queries,
            "cooldown_ms": config.cooldown.as_millis().to_string(),
            "pass_settle_ms": config.pass_settle.as_millis().to_string(),
        }),
    };
    artifact.write_to(out)?;
    Ok(())
}

#[expect(
    clippy::print_stderr,
    reason = "warm-matrix probe reports failures on stderr by design"
)]
fn main() -> ExitCode {
    let out = parse_out_path();
    match run(&out) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("dsl_warm_matrix: {err:#}");
            ExitCode::FAILURE
        }
    }
}
