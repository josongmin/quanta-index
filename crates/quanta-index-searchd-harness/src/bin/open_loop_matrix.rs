//! Open-loop offered-load matrix over the runtime harness query socket.

#[path = "../open_loop.rs"]
mod open_loop;

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use anyhow::{Context, Result as AnyResult};
use quanta_index_searchd_harness::artifact::{GitHeadV1, HostV1};

fn parse_args() -> AnyResult<(open_loop::Config, PathBuf)> {
    let mut config = open_loop::Config {
        seed: 0x4f50_454e_4c4f_4f50,
        rates_qps: vec![25, 50, 100, 200],
        duration: Duration::from_secs(10),
        workers: 32,
        queue_capacity: 256,
        request_timeout: Duration::from_secs(2),
    };
    let mut out_dir = PathBuf::from("artifacts/search-quality/open-loop/latest");
    let mut args = std::env::args().skip(1);
    while let Some(flag) = args.next() {
        let raw = args
            .next()
            .with_context(|| format!("{flag} requires a value"))?;
        match flag.as_str() {
            "--seed" => config.seed = raw.parse()?,
            "--rates-qps" => {
                config.rates_qps = raw
                    .split(',')
                    .map(str::parse)
                    .collect::<Result<Vec<u32>, _>>()?
            }
            "--duration-ms" => config.duration = Duration::from_millis(raw.parse()?),
            "--workers" => config.workers = raw.parse()?,
            "--queue-capacity" => config.queue_capacity = raw.parse()?,
            "--request-timeout-ms" => config.request_timeout = Duration::from_millis(raw.parse()?),
            "--out-dir" => out_dir = PathBuf::from(raw),
            _ => anyhow::bail!("unknown argument {flag:?}"),
        }
    }
    config.validate()?;
    Ok((config, out_dir))
}

fn run(config: open_loop::Config, out_dir: &Path) -> AnyResult<open_loop::Report> {
    let git_head = GitHeadV1::resolve(Path::new("."))?;
    let host = HostV1::observe()?;
    let report = open_loop::run(config)?;
    open_loop::artifact(&report, git_head, host)?.write_to(&out_dir.join("summary.json"))?;
    Ok(report)
}

#[expect(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "benchmark CLI reports measured results"
)]
fn main() -> ExitCode {
    let (config, out_dir) = match parse_args() {
        Ok(value) => value,
        Err(error) => {
            eprintln!("open_loop_matrix: {error:#}");
            return ExitCode::FAILURE;
        }
    };
    let report = match run(config, &out_dir) {
        Ok(report) => report,
        Err(error) => {
            eprintln!("open_loop_matrix: {error:#}");
            return ExitCode::FAILURE;
        }
    };
    for point in &report.points {
        println!(
            "open_loop[qps{}]: offered={} served={} offered_qps={:.2} achieved_qps={:.2} p50_ms={:.3} p95_ms={:.3} p99_ms={:.3} typed_errors={} unexpected_typed_errors={} timeouts={} transport_errors={} invalid_results={} drops={} saturated={}",
            point.target_qps,
            point.offered,
            point.served,
            point.offered_qps,
            point.achieved_qps,
            point.latency.map_or(0.0, |value| value.p50_ms),
            point.latency.map_or(0.0, |value| value.p95_ms),
            point.latency.map_or(0.0, |value| value.p99_ms),
            point.typed_errors,
            point.unexpected_typed_errors,
            point.timeouts,
            point.transport_errors,
            point.invalid_results,
            point.dropped_queue_full + point.dropped_scheduler_late + point.dropped_deadline,
            point.saturated
        );
    }
    println!(
        "open_loop: saturation_onset_qps={:?}",
        report.saturation_onset_qps()
    );
    if !report.passed() {
        eprintln!(
            "open_loop_matrix: correctness failure: no healthy first load point, invalid result, or unexpected typed error"
        );
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}
