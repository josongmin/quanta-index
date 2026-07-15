//! `tail_matrix` — latency-tail rail artifact producer + gate (J7Q-04).
//!
//! Boots one warm `E2eRuntime`, golden-validates each budgeted route's
//! representative query, then times it [`TAIL_SAMPLES`] times to read p50/p95/p99
//! off a sorted sample, and writes the canonical artifacts under
//! `artifacts/search-quality/tail/latest/`. Per-route budgets are declared in
//! `route_budgets.json`; `summary.json` records the measured tails with
//! route-local diagnostics. Authority behind `just rust-verify-quality-tail`.
//!
//! Fail-closed: a route that fails golden validation (wrong shape/count/error)
//! is a non-zero exit, never a fast-but-wrong "pass". Latency budgets are
//! advisory on this host this increment (canonical blocking is the Linux runner).

use std::path::PathBuf;
use std::process::ExitCode;

use quanta_index_searchd_harness::tail::{run_tail_report, write_artifacts};

fn git_rev() -> String {
    if let Ok(rev) = std::env::var("DSL_BENCH_GIT_REV") {
        return rev;
    }
    "unknown".to_string()
}

fn parse_out_dir() -> PathBuf {
    let mut args = std::env::args().skip(1);
    match (args.next().as_deref(), args.next()) {
        (Some("--out-dir"), Some(path)) => PathBuf::from(path),
        _ => PathBuf::from("artifacts/search-quality/tail/latest"),
    }
}

#[expect(
    clippy::print_stderr,
    clippy::print_stdout,
    reason = "rail binary reports rail status on stdout/stderr by design"
)]
fn main() -> ExitCode {
    let out_dir = parse_out_dir();
    let rev = git_rev();

    let report = match run_tail_report() {
        Ok(report) => report,
        Err(err) => {
            eprintln!("tail_matrix: rail run failed: {err:#}");
            return ExitCode::FAILURE;
        }
    };

    if let Err(err) = write_artifacts(&report, &out_dir, &rev) {
        eprintln!(
            "tail_matrix: failed to write artifacts under {}: {err:#}",
            out_dir.display()
        );
        return ExitCode::FAILURE;
    }

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
