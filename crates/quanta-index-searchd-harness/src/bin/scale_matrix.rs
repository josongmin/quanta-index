//! `scale_matrix` — scale-tier rail artifact producer + gate (J7Q-03).
//!
//! Emits the checked-in tier manifest, measures the SMALL tier end-to-end
//! (build -> activate -> cold and warm queries -> adapter-only open / plan /
//! execute -> one-file delta -> reclaim) via the real `E2eRuntime`, and writes
//! the canonical artifacts under `artifacts/search-quality/scale/latest/`:
//! `tier_manifest.json` and `summary.json`, the `BenchArtifactV1` (QI-BB-010)
//! naming the exact head of a clean worktree, the generated corpus digest, the
//! tier parameters, the host, the process's peak RSS, the build / update /
//! reclaim phases and the build's disk amplification. Medium/large/xlarge are
//! emitted as `declared-advisory`: their blocking latency is owned by the
//! canonical Linux perf runner, not this host. Authority behind
//! `just rust-verify-quality-scale`.
//!
//! Fail-closed: an empty or typed-error small-tier query is a non-zero exit,
//! not a fabricated zero-latency pass; a dirty tree or an unresolvable head
//! refuses to write.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::Result as AnyResult;
use quanta_index_searchd_harness::artifact::{GitHeadV1, HostV1};
use quanta_index_searchd_harness::scale::{TierMeasurement, measure_small_tier, write_artifacts};

/// Deterministic default seed so the rail is reproducible run-to-run unless an
/// operator overrides it via `--seed`.
const DEFAULT_SEED: u64 = 0x5161_5343_414c_4531;

struct CliArgs {
    out_dir: PathBuf,
    seed: u64,
}

fn parse_args() -> AnyResult<CliArgs> {
    let mut out_dir = PathBuf::from("artifacts/search-quality/scale/latest");
    let mut seed = DEFAULT_SEED;
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
            other => return Err(anyhow::anyhow!("unknown argument {other:?}")),
        }
    }
    Ok(CliArgs { out_dir, seed })
}

fn run(cli: &CliArgs) -> AnyResult<TierMeasurement> {
    // Provenance first: a run that cannot be attributed is not started.
    let git_head = GitHeadV1::resolve(Path::new("."))?;
    let host = HostV1::observe()?;
    let measurement = measure_small_tier(cli.seed)?;
    write_artifacts(&measurement, &cli.out_dir, git_head, host)?;
    Ok(measurement)
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
            eprintln!("scale_matrix: {err:#}");
            return ExitCode::FAILURE;
        }
    };
    let measurement = match run(&cli) {
        Ok(measurement) => measurement,
        Err(err) => {
            eprintln!("scale_matrix: {err:#}");
            return ExitCode::FAILURE;
        }
    };
    println!(
        "scale[small]: seed={} files={} build_ms={:.3} activation_ms={:.3} first_query_ms={:.3} warm_p50_ms={:.3} daemon_cold_open_ms={:.0} adapter_open_ms={:.3} plan_ms={:.3} execute_ms={:.3} update_ms={:.3} reclaimed_bytes={} results={}",
        measurement.seed,
        measurement.file_count,
        measurement.build_ms,
        measurement.activation_ms,
        measurement.first_query_ms,
        measurement.warm_query.p50_ms,
        measurement.daemon.cold_open_ms,
        measurement.adapter.open_ms,
        measurement.adapter.plan_ms,
        measurement.adapter.execute_ms,
        measurement.delta.update_ms,
        measurement.delta.reclaimed_bytes,
        measurement.result_count,
    );
    println!("scale rail green (small measured; medium/large/xlarge declared-advisory)");
    ExitCode::SUCCESS
}
