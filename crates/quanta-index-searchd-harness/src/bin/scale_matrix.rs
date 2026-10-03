//! `scale_matrix` — scale-tier rail artifact producer + gate (J7Q-03).
//!
//! Emits the checked-in tier manifest and measures the selected tier end-to-end
//! (build -> activate -> cold and warm queries -> adapter-only open / plan /
//! execute -> one-file delta -> reclaim) via the real `E2eRuntime`, and writes
//! the canonical artifacts under `artifacts/search-quality/scale/latest/`:
//! `tier_manifest.json` and `summary.json`, the `BenchArtifactV1` (QI-BB-010)
//! naming the exact head of a clean worktree, the generated corpus digest, the
//! tier parameters, the host, the process's peak RSS, the build / update /
//! reclaim phases and the build's disk amplification. The default is small;
//! `--tier` or `--all-tiers` selects larger scoped source-repository fixtures.
//! Unselected tiers are advisory in each artifact. Authority behind
//! `just rust-verify-quality-scale`.
//!
//! Fail-closed: an empty or typed-error selected-tier query is a non-zero exit,
//! not a fabricated zero-latency pass; a dirty tree or an unresolvable head
//! refuses to write.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use anyhow::Result as AnyResult;
use quanta_index_ipc::DEFAULT_CLIENT_IO_TIMEOUT;
use quanta_index_searchd_harness::artifact::{GitHeadV1, HostV1};
use quanta_index_searchd_harness::scale::{
    ScaleTier, TierMeasurement, measure_tier_with_client_timeout, source_binding_for_failure,
    write_artifacts, write_refusal_artifact_with_context,
};
use serde_json::json;

/// Deterministic default seed so the rail is reproducible run-to-run unless an
/// operator overrides it via `--seed`.
const DEFAULT_SEED: u64 = 0x5161_5343_414c_4531;

struct CliArgs {
    out_dir: PathBuf,
    seed: u64,
    tiers: Vec<ScaleTier>,
    fresh_output: bool,
    client_timeout_ms: Option<u64>,
}

fn parse_client_timeout_ms(raw: &str) -> AnyResult<u64> {
    let parsed = raw.parse::<u64>()?;
    if !(1..=600_000).contains(&parsed) {
        anyhow::bail!("--client-timeout-ms must be in 1..=600000");
    }
    Ok(parsed)
}

fn parse_args() -> AnyResult<CliArgs> {
    let mut out_dir = PathBuf::from("artifacts/search-quality/scale/latest");
    let mut seed = DEFAULT_SEED;
    let mut tiers = vec![ScaleTier::Small];
    let mut out_dir_explicit = false;
    let mut client_timeout_ms = None;
    let mut args = std::env::args().skip(1);
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--out-dir" => {
                out_dir_explicit = true;
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
            "--client-timeout-ms" => {
                let raw = args
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--client-timeout-ms requires a value"))?;
                client_timeout_ms = Some(parse_client_timeout_ms(&raw)?);
            }
            "--tier" => {
                let raw = args
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--tier requires a tier"))?;
                tiers = vec![match raw.as_str() {
                    "small" => ScaleTier::Small,
                    "medium" => ScaleTier::Medium,
                    "large" => ScaleTier::Large,
                    "xlarge" => ScaleTier::Xlarge,
                    _ => anyhow::bail!("--tier must be small, medium, large or xlarge"),
                }];
            }
            "--all-tiers" => {
                tiers = vec![
                    ScaleTier::Small,
                    ScaleTier::Medium,
                    ScaleTier::Large,
                    ScaleTier::Xlarge,
                ];
            }
            other => return Err(anyhow::anyhow!("unknown argument {other:?}")),
        }
    }
    if !out_dir_explicit && (tiers.len() != 1 || tiers[0] != ScaleTier::Small) {
        anyhow::bail!("--out-dir is required for a non-default scale tier");
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
    Ok(CliArgs {
        out_dir,
        seed,
        tiers,
        fresh_output: out_dir_explicit,
        client_timeout_ms,
    })
}

fn run(cli: &CliArgs) -> AnyResult<Vec<TierMeasurement>> {
    // Provenance first: a run that cannot be attributed is not started.
    let git_head = GitHeadV1::resolve(Path::new("."))?;
    let host = HostV1::observe()?;
    if cli.fresh_output {
        if let Some(parent) = cli.out_dir.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::create_dir(&cli.out_dir)?;
    }
    let mut measurements = Vec::with_capacity(cli.tiers.len());
    for tier in &cli.tiers {
        match measure_tier_with_client_timeout(
            *tier,
            cli.seed,
            cli.client_timeout_ms.map(Duration::from_millis),
        ) {
            Ok(measurement) => measurements.push(measurement),
            Err(error) => {
                let binding = source_binding_for_failure(*tier, cli.seed)?;
                let execution = json!({
                    "client_request_timeout_ms": cli.client_timeout_ms.unwrap_or(
                        u64::try_from(DEFAULT_CLIENT_IO_TIMEOUT.as_millis())?
                    ),
                });
                write_refusal_artifact_with_context(
                    &binding,
                    &cli.out_dir,
                    &git_head,
                    &host,
                    &error,
                    "scale",
                    Some(&execution),
                )?;
                return Err(error);
            }
        }
    }
    // An all-tier run does not write earlier tier artifacts if a later tier
    // refuses source or wire admission.
    for measurement in &measurements {
        let out_dir = if cli.tiers.len() == 1 {
            cli.out_dir.clone()
        } else {
            cli.out_dir.join(measurement.tier.as_str())
        };
        write_artifacts(measurement, &out_dir, git_head.clone(), host.clone())?;
    }
    Ok(measurements)
}

fn format_cold_open_ms(value: Option<f64>) -> String {
    value.map_or_else(|| "unavailable".to_string(), |ms| format!("{ms:.0}"))
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
    let measurements = match run(&cli) {
        Ok(measurements) => measurements,
        Err(err) => {
            eprintln!("scale_matrix: {err:#}");
            return ExitCode::FAILURE;
        }
    };
    for measurement in &measurements {
        println!(
            "scale[{}]: seed={} serving_owners=1 source_repos={} files={} build_ms={:.3} activation_ms={:.3} first_query_ms={:.3} warm_p50_ms={:.3} daemon_cold_open_ms={} adapter_open_ms={:.3} plan_ms={:.3} execute_ms={:.3} update_ms={:.3} reclaimed_bytes={} results={}",
            measurement.tier.as_str(),
            measurement.seed,
            measurement.source_repo_count,
            measurement.file_count,
            measurement.build_ms,
            measurement.activation_ms,
            measurement.first_query_ms,
            measurement.warm_query.p50_ms,
            format_cold_open_ms(measurement.daemon.cold_open_ms),
            measurement.adapter.open_ms,
            measurement.adapter.plan_ms,
            measurement.adapter.execute_ms,
            measurement.delta.update_ms,
            measurement.delta.reclaimed_bytes,
            measurement.result_count,
        );
        if let Some(cpu) = measurement.cpu {
            println!(
                "scale[{}] process_cpu: user_ms={:.3} system_ms={:.3} scope=harness+daemon-thread",
                measurement.tier.as_str(),
                cpu.user_ms,
                cpu.system_ms,
            );
        }
        if let Some(delete) = measurement.delete_reopen {
            println!(
                "scale[{}] delete_reopen: delete_seal_ms={:.3} activation_ms={:.3} same_process_reopen_ms={:.3} reopened_first_query_ms={:.3}",
                measurement.tier.as_str(),
                delete.delete_seal_ms,
                delete.delete_activation_ms,
                delete.same_process_reopen_ms,
                delete.reopened_first_query_ms,
            );
        }
    }
    println!(
        "scale rail green (selected tiers measured; canonical performance qualification separate)"
    );
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::{format_cold_open_ms, parse_client_timeout_ms};

    #[test]
    fn absent_query_cold_open_is_not_printed_as_zero() {
        assert_eq!(format_cold_open_ms(None), "unavailable");
        assert_eq!(format_cold_open_ms(Some(0.0)), "0");
    }

    #[test]
    fn scale_client_timeout_override_is_explicit_and_bounded() {
        assert_eq!(
            parse_client_timeout_ms("300000").expect("bounded timeout"),
            300_000
        );
        for invalid in ["0", "600001", "nan", "-1"] {
            assert!(parse_client_timeout_ms(invalid).is_err());
        }
    }
}
