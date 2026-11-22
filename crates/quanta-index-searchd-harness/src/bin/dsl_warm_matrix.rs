//! Warm DSL matrix entry point for Native file samples.

use std::path::Path;
use std::process::ExitCode;
use std::time::Duration;

use anyhow::Result as AnyResult;
use quanta_index_searchd_harness::artifact::BenchMode;
use quanta_index_searchd_harness::native_index::{
    NativeDslTiming, assemble_native_dsl_artifact, measure_native_dsl_history,
    measure_native_dsl_retained, measure_native_dsl_text, measure_reopened_native_dsl_history,
    native_dsl_scenario_roster,
};

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

/// Shared sampling options for the DSL warm matrix.
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
}

fn run_cli() -> AnyResult<()> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if let [flag, input_flag, input, out_flag, out] = args.as_slice()
        && flag == "--assemble-native"
        && input_flag == "--input"
        && out_flag == "--out"
    {
        assemble_native_dsl_artifact(Path::new(input), Path::new(out), BenchMode::Warm)?;
        return Ok(());
    }
    if args.as_slice() == ["--list-native"] {
        serde_json::to_writer(std::io::stdout().lock(), &native_dsl_scenario_roster())?;
        return Ok(());
    }
    if let Some(flag) = args.first()
        && (flag == "--native-text"
            || flag == "--native-history"
            || flag == "--native-retained"
            || flag == "--native-history-reopen")
    {
        let [_, input, id] = args.as_slice() else {
            anyhow::bail!("{flag:?} requires <daemon-or-fixture-input> <scenario-id>");
        };
        let id = id
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("Native DSL scenario ID must be UTF-8"))?;
        let config = WarmConfig::from_env()?;
        let timing = NativeDslTiming {
            passes: config.repeats,
            samples: config.samples,
            prime_queries: config.prime_queries,
            cooldown: config.cooldown,
            pass_settle: config.pass_settle,
        };
        let report = match flag.to_str() {
            Some("--native-text") => measure_native_dsl_text(Path::new(input), id, timing)?,
            Some("--native-retained") => measure_native_dsl_retained(Path::new(input), id, timing)?,
            Some("--native-history") => measure_native_dsl_history(Path::new(input), id, timing)?,
            Some("--native-history-reopen") => {
                measure_reopened_native_dsl_history(Path::new(input), id, timing)?
            }
            _ => unreachable!("checked Native DSL mode"),
        };
        serde_json::to_writer(std::io::stdout().lock(), &report)?;
        return Ok(());
    }
    anyhow::bail!(
        "usage: dsl_warm_matrix (--list-native | --native-text <daemon> <scenario-id> | --native-retained <daemon> <scenario-id> | --native-history <fixture-input-json> <scenario-id> | --native-history-reopen <cold-fixture-json> <scenario-id> | --assemble-native --input <matrix.json> --out <artifact.json>)"
    );
}

#[expect(
    clippy::print_stderr,
    reason = "warm-matrix probe reports failures on stderr by design"
)]
fn main() -> ExitCode {
    match run_cli() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("dsl_warm_matrix: {err:#}");
            ExitCode::FAILURE
        }
    }
}
