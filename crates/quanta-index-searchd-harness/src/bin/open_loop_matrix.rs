//! Open-loop offered-load matrix over the runtime harness query socket.

#[path = "../open_loop.rs"]
mod open_loop;

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use anyhow::{Context, Result as AnyResult};
use quanta_index_searchd_harness::artifact::{GitHeadV1, HostV1};
use quanta_index_searchd_harness::scale::{
    ScaleRuntimeConfig, ScaleTier, source_binding_for_failure_in_dimension,
    write_refusal_artifact_with_context,
};
use serde_json::{Value, json};

const USAGE: &str = "Usage: open_loop_matrix [--tier small|medium|large|xlarge]
    [--seed U64] [--arrival-model seeded-poisson|deterministic-periodic]
    [--rates-qps COMMA_SEPARATED_U32] [--duration-ms U64] [--workers USIZE]
    [--queue-capacity USIZE] [--request-timeout-ms U64]
    [--history-max-bytes 1..=268435456] [--out-dir ABSOLUTE_EXTERNAL_NEW_PATH]
    Default tier: small (16 files). medium=256, large=4096, xlarge=32768.
    Default history profile is separate from explicit diagnostic overrides.
    --help, -h  Print this usage without running the rail.";

fn execution_context(config: &open_loop::Config) -> AnyResult<Value> {
    Ok(json!({
        "arrival_model": config.arrival_model.as_str(),
        "rates_qps": config.rates_qps,
        "duration_ms": config.duration.as_millis(),
        "workers": config.workers,
        "queue_capacity": config.queue_capacity,
        "request_timeout_ms": config.request_timeout.as_millis(),
        "history_policy": config.history_policy_json()?,
    }))
}

fn parse_history_max_bytes(raw: &str) -> AnyResult<u64> {
    let bytes = raw.parse::<u64>()?;
    ScaleRuntimeConfig {
        client_timeout: None,
        history_max_bytes: Some(bytes),
    }
    .effective_history_max_bytes()
}

fn parse_args() -> AnyResult<(open_loop::Config, PathBuf, bool)> {
    let mut config = open_loop::Config {
        seed: 0x4f50_454e_4c4f_4f50,
        tier: ScaleTier::Small,
        arrival_model: open_loop::ArrivalModel::SeededPoisson,
        rates_qps: vec![25, 50, 100, 200],
        duration: Duration::from_secs(10),
        workers: 32,
        queue_capacity: 256,
        request_timeout: Duration::from_secs(2),
        history_max_bytes: None,
    };
    let mut out_dir = PathBuf::from("artifacts/search-quality/open-loop/latest");
    let mut out_dir_explicit = false;
    let mut args = std::env::args().skip(1);
    while let Some(flag) = args.next() {
        let raw = args
            .next()
            .with_context(|| format!("{flag} requires a value"))?;
        match flag.as_str() {
            "--seed" => config.seed = raw.parse()?,
            "--tier" => {
                config.tier = match raw.as_str() {
                    "small" => ScaleTier::Small,
                    "medium" => ScaleTier::Medium,
                    "large" => ScaleTier::Large,
                    "xlarge" => ScaleTier::Xlarge,
                    _ => anyhow::bail!("--tier must be small, medium, large or xlarge"),
                }
            }
            "--arrival-model" => {
                config.arrival_model = match raw.as_str() {
                    "seeded-poisson" => open_loop::ArrivalModel::SeededPoisson,
                    "deterministic-periodic" => open_loop::ArrivalModel::DeterministicPeriodic,
                    _ => anyhow::bail!(
                        "--arrival-model must be seeded-poisson or deterministic-periodic"
                    ),
                }
            }
            "--rates-qps" => {
                config.rates_qps = raw
                    .split(',')
                    .map(str::parse)
                    .collect::<Result<Vec<u32>, _>>()?;
            }
            "--duration-ms" => config.duration = Duration::from_millis(raw.parse()?),
            "--workers" => config.workers = raw.parse()?,
            "--queue-capacity" => config.queue_capacity = raw.parse()?,
            "--request-timeout-ms" => config.request_timeout = Duration::from_millis(raw.parse()?),
            "--history-max-bytes" => {
                config.history_max_bytes = Some(parse_history_max_bytes(&raw)?);
            }
            "--out-dir" => {
                out_dir = PathBuf::from(raw);
                out_dir_explicit = true;
            }
            _ => anyhow::bail!("unknown argument {flag:?}"),
        }
    }
    if config.tier != ScaleTier::Small && !out_dir_explicit {
        anyhow::bail!("--out-dir is required for a non-default open-loop tier");
    }
    if out_dir_explicit && out_dir.exists() {
        anyhow::bail!("--out-dir must name a new output root; refusing to overwrite artifacts");
    }
    if out_dir_explicit {
        if !out_dir.is_absolute() {
            anyhow::bail!("--out-dir must be an absolute external path");
        }
        let parent = out_dir
            .parent()
            .ok_or_else(|| anyhow::anyhow!("--out-dir has no parent"))?;
        let parent = std::fs::canonicalize(parent)?;
        let checkout = std::fs::canonicalize(".")?;
        if parent.starts_with(checkout) {
            anyhow::bail!("--out-dir must be outside the checkout");
        }
    }
    config.validate()?;
    Ok((config, out_dir, out_dir_explicit))
}

fn run(
    config: &open_loop::Config,
    out_dir: &Path,
    fresh_output: bool,
) -> AnyResult<open_loop::Report> {
    let git_head = GitHeadV1::resolve(Path::new("."))?;
    let host = HostV1::observe()?;
    if fresh_output {
        std::fs::create_dir(out_dir)?;
    }
    let report = match open_loop::run(config.clone()) {
        Ok(report) => report,
        Err(error) => {
            let binding = source_binding_for_failure_in_dimension(
                open_loop::DIMENSION,
                config.tier,
                config.seed,
            )?;
            let execution = execution_context(config)?;
            write_refusal_artifact_with_context(
                &binding,
                out_dir,
                &git_head,
                &host,
                &error,
                open_loop::DIMENSION,
                Some(&execution),
            )?;
            return Err(error);
        }
    };
    open_loop::artifact(&report, git_head, host)?.write_to(&out_dir.join("summary.json"))?;
    Ok(report)
}

fn format_latency(value: Option<f64>) -> String {
    value.map_or_else(|| "unavailable".to_string(), |ms| format!("{ms:.3}"))
}

#[expect(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "benchmark CLI reports measured results"
)]
fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if matches!(args.as_slice(), [arg] if matches!(arg.as_str(), "--help" | "-h")) {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    let (config, out_dir, fresh_output) = match parse_args() {
        Ok(value) => value,
        Err(error) => {
            eprintln!("open_loop_matrix: {error:#}");
            return ExitCode::FAILURE;
        }
    };
    let report = match run(&config, &out_dir, fresh_output) {
        Ok(report) => report,
        Err(error) => {
            eprintln!("open_loop_matrix: {error:#}");
            return ExitCode::FAILURE;
        }
    };
    for point in &report.points {
        println!(
            "open_loop[qps{}]: offered={} served={} offered_qps={:.2} achieved_qps={:.2} p50_ms={} p95_ms={} p99_ms={} typed_errors={} unexpected_typed_errors={} timeouts={} transport_errors={} invalid_results={} drops={} saturated={}",
            point.target_qps,
            point.offered,
            point.served,
            point.offered_qps,
            point.achieved_qps,
            format_latency(point.latency.map(|value| value.p50_ms)),
            format_latency(point.latency.map(|value| value.p95_ms)),
            format_latency(point.latency.map(|value| value.p99_ms)),
            point.typed_errors,
            point.unexpected_typed_errors,
            point.timeouts,
            point.transport_errors,
            point.invalid_results,
            point
                .dropped_queue_full
                .saturating_add(point.dropped_scheduler_late)
                .saturating_add(point.dropped_deadline),
            point.saturated
        );
    }
    println!(
        "open_loop: saturation_onset_qps={:?}",
        report.saturation_onset_qps()
    );
    if report.passed() {
        ExitCode::SUCCESS
    } else {
        eprintln!(
            "open_loop_matrix: correctness failure: no healthy first load point, invalid result, or unexpected typed error"
        );
        ExitCode::FAILURE
    }
}

#[cfg(test)]
mod tests {
    use super::{execution_context, format_latency, open_loop, parse_history_max_bytes};
    use quanta_index_searchd_harness::artifact::{GitHeadV1, HostV1};
    use quanta_index_searchd_harness::scale::{
        ScaleStageError, ScaleTier, refusal_json_with_context,
        source_binding_for_failure_in_dimension,
    };
    use std::time::Duration;

    #[test]
    fn unavailable_latency_is_not_printed_as_zero() {
        assert_eq!(format_latency(None), "unavailable");
        assert_eq!(format_latency(Some(0.0)), "0.000");
    }

    #[test]
    fn refusal_context_names_the_exact_arrival_contract() {
        let config = open_loop::Config {
            seed: 7,
            tier: ScaleTier::Medium,
            arrival_model: open_loop::ArrivalModel::SeededPoisson,
            rates_qps: vec![25, 50],
            duration: Duration::from_secs(10),
            workers: 4,
            queue_capacity: 8,
            request_timeout: Duration::from_millis(250),
            history_max_bytes: None,
        };
        let value = execution_context(&config).expect("valid execution context");
        assert_eq!(value["arrival_model"], "seeded_poisson");
        assert_eq!(value["rates_qps"], serde_json::json!([25, 50]));
        assert_eq!(value["duration_ms"], 10_000);
        assert_eq!(value["workers"], 4);
        assert_eq!(value["queue_capacity"], 8);
        assert_eq!(value["request_timeout_ms"], 250);
        assert_eq!(value["history_policy"]["history_max_bytes"], 16_777_216);
        assert!(value["history_policy"]["requested_history_max_bytes"].is_null());
        assert_eq!(value["history_policy"]["history_max_generations"], 8);
    }

    #[test]
    fn explicit_history_budget_is_bounded_and_bound_to_refusal_context() {
        for invalid in ["0", "268435457", "-1", "nan"] {
            assert!(parse_history_max_bytes(invalid).is_err());
        }
        let bytes = parse_history_max_bytes("268435456").expect("harness total cap");
        let config = open_loop::Config {
            seed: 7,
            tier: ScaleTier::Large,
            arrival_model: open_loop::ArrivalModel::SeededPoisson,
            rates_qps: vec![25],
            duration: Duration::from_secs(1),
            workers: 1,
            queue_capacity: 8,
            request_timeout: Duration::from_millis(250),
            history_max_bytes: Some(bytes),
        };
        config.validate().expect("explicit bounded budget");
        let value = execution_context(&config).expect("failure execution context");
        assert_eq!(value["history_policy"]["history_max_bytes"], bytes);
        assert_eq!(
            value["history_policy"]["requested_history_max_bytes"],
            bytes
        );
        assert_eq!(value["history_policy"]["history_max_total_bytes"], bytes);
        assert_eq!(value["history_policy"]["history_max_revision_pairs"], 128);
    }

    #[test]
    fn refusal_artifact_keeps_requested_policy_and_original_operation_stage() -> anyhow::Result<()>
    {
        let config = open_loop::Config {
            seed: 7,
            tier: ScaleTier::Medium,
            arrival_model: open_loop::ArrivalModel::SeededPoisson,
            rates_qps: vec![25],
            duration: Duration::from_secs(1),
            workers: 1,
            queue_capacity: 8,
            request_timeout: Duration::from_millis(250),
            history_max_bytes: Some(268_435_456),
        };
        let binding = source_binding_for_failure_in_dimension(
            open_loop::DIMENSION,
            config.tier,
            config.seed,
        )?;
        let head = GitHeadV1::parse("0123456789abcdef0123456789abcdef01234567")?;
        let host = HostV1 {
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
            cpu_count: 4,
            mem_bytes: 1 << 30,
            hostname_hash: "sha256:host".to_string(),
        };
        let failure = anyhow::Error::new(ScaleStageError::operation(
            "build_seal",
            &anyhow::anyhow!("fixed retention fault"),
        ));
        let execution = execution_context(&config)?;
        let refusal = refusal_json_with_context(
            &binding,
            &head,
            &host,
            &failure,
            open_loop::DIMENSION,
            Some(&execution),
        );
        assert_eq!(refusal["status"], "failed");
        assert_eq!(refusal["failure"]["stage"], "build_seal");
        assert_eq!(
            refusal["failure"]["message"],
            "scale build_seal: fixed retention fault"
        );
        assert_eq!(
            refusal["execution"]["history_policy"]["history_max_bytes"],
            268_435_456
        );
        assert_eq!(
            refusal["execution"]["history_policy"]["requested_history_max_bytes"],
            268_435_456
        );
        assert!(refusal.get("latency").is_none());
        Ok(())
    }
}
