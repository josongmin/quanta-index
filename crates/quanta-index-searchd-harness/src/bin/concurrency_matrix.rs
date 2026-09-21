//! `concurrency_matrix` — concurrent-client rail artifact producer + gate
//! (QI-BB-010 #4).
//!
//! Serves one sealed generation of the seeded medium corpus and runs 1 / 8 /
//! 32 fast clients over the mixed lexical / semantic / hybrid / count route
//! set, with one slow page-maximum client mixed in above one client, then
//! writes one `BenchArtifactV1` per client count under
//! `artifacts/search-quality/concurrency/latest/`: the exact head of a clean
//! worktree, the corpus and configuration digests, the host, the process's
//! peak RSS, and per-route p50/p95/p99, QPS, error and timeout counts.
//!
//! Fail-closed: a request the daemon never answered (a transport failure
//! that is not a timeout), a route the fixture cannot serve, a dirty tree or
//! an unresolvable head is a non-zero exit. The rail's blocking signal on
//! this host is that every request was answered and nothing timed out;
//! latency is recorded against the host, never gated here.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::Result as AnyResult;
use quanta_index_searchd_harness::artifact::{GitHeadV1, HostV1};
use quanta_index_searchd_harness::concurrency::{
    ConcurrencyReport, run_concurrency_report, write_artifacts,
};

/// Deterministic default seed so the rail is reproducible run-to-run unless an
/// operator overrides it via `--seed`.
const DEFAULT_SEED: u64 = 0x434f_4e43_5552_5231;

/// Requests each fast client issues per client count; small enough to run on
/// a developer host, overridable via `--requests-per-client`.
const DEFAULT_REQUESTS_PER_CLIENT: u32 = 16;

struct CliArgs {
    out_dir: PathBuf,
    seed: u64,
    requests_per_client: u32,
}

fn parse_args() -> AnyResult<CliArgs> {
    let mut out_dir = PathBuf::from("artifacts/search-quality/concurrency/latest");
    let mut seed = DEFAULT_SEED;
    let mut requests_per_client = DEFAULT_REQUESTS_PER_CLIENT;
    let mut args = std::env::args().skip(1);
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--out-dir" => {
                out_dir = PathBuf::from(
                    args.next()
                        .ok_or_else(|| anyhow::anyhow!("--out-dir requires a path"))?,
                );
            }
            "--seed" => {
                let raw = args
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--seed requires a value"))?;
                seed = raw
                    .parse::<u64>()
                    .map_err(|err| anyhow::anyhow!("--seed {raw:?}: {err}"))?;
            }
            "--requests-per-client" => {
                let raw = args
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--requests-per-client requires a value"))?;
                requests_per_client = raw
                    .parse::<u32>()
                    .map_err(|err| anyhow::anyhow!("--requests-per-client {raw:?}: {err}"))?;
            }
            other => return Err(anyhow::anyhow!("unknown argument {other:?}")),
        }
    }
    Ok(CliArgs {
        out_dir,
        seed,
        requests_per_client,
    })
}

fn run(cli: &CliArgs) -> AnyResult<ConcurrencyReport> {
    // Provenance first: a run that cannot be attributed is not started.
    let git_head = GitHeadV1::resolve(Path::new("."))?;
    let host = HostV1::observe()?;
    let report = run_concurrency_report(cli.seed, cli.requests_per_client)?;
    write_artifacts(&report, &cli.out_dir, &git_head, &host)?;
    Ok(report)
}

#[expect(
    clippy::print_stderr,
    clippy::print_stdout,
    reason = "rail binary reports rail status on stdout/stderr by design"
)]
fn main() -> ExitCode {
    let cli = match parse_args() {
        Ok(cli) => cli,
        Err(err) => {
            eprintln!("concurrency_matrix: {err:#}");
            return ExitCode::FAILURE;
        }
    };
    let report = match run(&cli) {
        Ok(report) => report,
        Err(err) => {
            eprintln!("concurrency_matrix: {err:#}");
            return ExitCode::FAILURE;
        }
    };
    for measurement in &report.measurements {
        let (p50, p95, p99) = measurement
            .fast
            .latency
            .map_or((0.0, 0.0, 0.0), |latency| (latency.p50_ms, latency.p95_ms, latency.p99_ms));
        println!(
            "concurrency[c{}]: requests={} served={} errors={} timeouts={} qps={:.2} p50_ms={:.3} p95_ms={:.3} p99_ms={:.3} slow_served={}",
            measurement.clients,
            measurement.fast.requests,
            measurement.fast.served,
            measurement.fast.error_count,
            measurement.fast.timeout_count,
            measurement.fast.qps,
            p50,
            p95,
            p99,
            measurement.slow.as_ref().map_or(0, |slow| slow.served),
        );
    }
    if report.passed {
        println!(
            "concurrency rail green (every request answered, no timeouts; latency advisory on this host)"
        );
        ExitCode::SUCCESS
    } else {
        eprintln!("concurrency rail RED: a request timed out");
        ExitCode::FAILURE
    }
}
