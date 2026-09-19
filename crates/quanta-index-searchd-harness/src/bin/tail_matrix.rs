//! `tail_matrix` — latency-tail rail artifact producer + gate (J7Q-04).
//!
//! Boots one warm `E2eRuntime`, golden-validates each budgeted route's
//! representative query, then times it `TAIL_SAMPLES` times to read p50/p95/p99
//! off a sorted sample, and writes the canonical artifacts under
//! `artifacts/search-quality/tail/latest/`. Per-route budgets are declared in
//! `route_budgets.json`; `summary.json` is the `BenchArtifactV1` (QI-BB-010):
//! the exact head of a clean worktree, the warm fixture corpus digest, the
//! sample configuration digest, the host and the process's peak RSS, with
//! the measured tails under `detail`. Authority behind
//! `just rust-verify-quality-tail`.
//!
//! Fail-closed: a route that fails golden validation (wrong shape/count/error)
//! is a non-zero exit, never a fast-but-wrong "pass"; a dirty tree or an
//! unresolvable head refuses to write. Latency budgets are advisory on this
//! host this increment (canonical blocking is the Linux runner).

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::Result as AnyResult;
use quanta_index_searchd_harness::artifact::{GitHeadV1, HostV1};
use quanta_index_searchd_harness::tail::{TailReport, run_tail_report, write_artifacts};

fn parse_out_dir() -> PathBuf {
    let mut args = std::env::args().skip(1);
    match (args.next().as_deref(), args.next()) {
        (Some("--out-dir"), Some(path)) => PathBuf::from(path),
        _ => PathBuf::from("artifacts/search-quality/tail/latest"),
    }
}

fn run(out_dir: &Path) -> AnyResult<TailReport> {
    // Provenance first: a run that cannot be attributed is not started.
    let git_head = GitHeadV1::resolve(Path::new("."))?;
    let host = HostV1::observe()?;
    let report = run_tail_report()?;
    write_artifacts(&report, out_dir, git_head, host)?;
    Ok(report)
}

#[expect(
    clippy::print_stderr,
    clippy::print_stdout,
    reason = "rail binary reports rail status on stdout/stderr by design"
)]
fn main() -> ExitCode {
    let out_dir = parse_out_dir();
    let report = match run(&out_dir) {
        Ok(report) => report,
        Err(err) => {
            eprintln!("tail_matrix: {err:#}");
            return ExitCode::FAILURE;
        }
    };

    for measurement in &report.measurements {
        println!(
            "tail[{}] scenario={} samples={} p50_ms={:.3} p95_ms={:.3} p99_ms={:.3}",
            measurement.route.as_str(),
            measurement.scenario_id,
            measurement.sample_count,
            measurement.p50_ms,
            measurement.p95_ms,
            measurement.p99_ms,
        );
    }

    if report.passed {
        println!(
            "tail rail green (route correctness validated; latency budgets advisory on this host)"
        );
        ExitCode::SUCCESS
    } else {
        eprintln!("tail rail RED: a budgeted route was not measured");
        ExitCode::FAILURE
    }
}
