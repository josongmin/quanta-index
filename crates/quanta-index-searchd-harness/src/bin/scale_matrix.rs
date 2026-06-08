//! `scale_matrix` — scale-tier rail artifact producer + gate (J7Q-03, first
//! increment).
//!
//! Emits the checked-in tier manifest, measures the SMALL tier end-to-end
//! (ingest -> seal -> activate -> one query) via the real `E2eRuntime`, captures
//! ingest / open / query wall-times, and writes the canonical artifacts under
//! `artifacts/search-quality/scale/latest/`. Medium/large/xlarge are emitted as
//! `declared-advisory`: their blocking latency is owned by the canonical Linux
//! perf runner, not this host. Authority behind `just rust-verify-quality-scale`.
//!
//! Fail-closed: an empty or typed-error small-tier query is a non-zero exit, not
//! a fabricated zero-latency pass.

use std::path::PathBuf;
use std::process::ExitCode;

use quanta_index_searchd_harness::scale::{measure_small_tier, write_artifacts};

/// Deterministic default seed so the rail is reproducible run-to-run unless an
/// operator overrides it via `--seed`.
const DEFAULT_SEED: u64 = 0x5161_5343_414c_4531;

fn git_rev() -> String {
    if let Ok(rev) = std::env::var("DSL_BENCH_GIT_REV") {
        return rev;
    }
    "unknown".to_string()
}

struct CliArgs {
    out_dir: PathBuf,
    seed: u64,
}

fn parse_args() -> CliArgs {
    let mut out_dir = PathBuf::from("artifacts/search-quality/scale/latest");
    let mut seed = DEFAULT_SEED;
    let mut args = std::env::args().skip(1);
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--out-dir" => {
                if let Some(path) = args.next() {
                    out_dir = PathBuf::from(path);
                }
            }
            "--seed" => {
                if let Some(value) = args.next()
                    && let Ok(parsed) = value.parse::<u64>()
                {
                    seed = parsed;
                }
            }
            _ => {}
        }
    }
    CliArgs { out_dir, seed }
}

#[expect(
    clippy::print_stderr,
    clippy::print_stdout,
    reason = "rail binary reports rail status on stdout/stderr by design"
)]
fn main() -> ExitCode {
    let cli = parse_args();
    let rev = git_rev();

    let measurement = match measure_small_tier(cli.seed) {
        Ok(measurement) => measurement,
        Err(err) => {
            eprintln!("scale_matrix: small-tier measurement failed: {err:#}");
            return ExitCode::FAILURE;
        }
    };

    if let Err(err) = write_artifacts(&measurement, &cli.out_dir, &rev) {
        eprintln!(
            "scale_matrix: failed to write artifacts under {}: {err:#}",
            cli.out_dir.display()
        );
        return ExitCode::FAILURE;
    }

    println!(
        "scale[small]: seed={} files={} ingest_ms={:.3} open_ms={:.3} query_ms={:.3} results={}",
        measurement.seed,
        measurement.file_count,
        measurement.ingest_ms,
        measurement.open_ms,
        measurement.query_ms,
        measurement.result_count,
    );
    println!("scale rail green (small measured; medium/large/xlarge declared-advisory)");
    ExitCode::SUCCESS
}
