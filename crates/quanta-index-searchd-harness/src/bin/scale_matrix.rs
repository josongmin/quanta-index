//! `scale_matrix` — scale-tier rail artifact producer + gate (J7Q-03).
//!
//! Emits the checked-in tier manifest and measures the selected tier end-to-end
//! (build -> activate -> cold and warm queries -> adapter-only open / plan /
//! execute -> one-file delta -> reclaim) via the real `E2eRuntime`, and writes
//! the canonical artifacts under `artifacts/search-quality/scale/latest/`:
//! `tier_manifest.json` and `summary.json`, the `BenchArtifactV1` (QI-BB-010)
//! naming the exact head of a clean worktree, the generated corpus digest, the
//! tier parameters, the host, the whole-process sampled maximum RSS, the build / update /
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
use quanta_index_core::{
    DiagnosticContextV1, DiagnosticLaneV1, DiagnosticRootKindV1, DiagnosticStageV1,
    DiagnosticTerminalV1,
};
use quanta_index_searchd::{DiagnosticCaptureLimitsV1, DiagnosticCaptureV1};
use quanta_index_searchd_harness::artifact::{GitHeadV1, HostV1};
use quanta_index_searchd_harness::scale::{
    DiskObservationMode, ScaleRuntimeConfig, ScaleTier, TierMeasurement,
    measure_tier_with_runtime_config_and_capture, source_binding_for_failure, write_artifacts,
    write_refusal_artifact_with_context,
};

/// Deterministic default seed so the rail is reproducible run-to-run unless an
/// operator overrides it via `--seed`.
const DEFAULT_SEED: u64 = 0x5161_5343_414c_4531;

const USAGE: &str = "Usage: scale_matrix [--tier small|medium|large|xlarge | --all-tiers]
    [--seed U64] [--client-timeout-ms 1..=600000]
    [--disk-observation boundary-only|periodic]
    [--history-max-bytes POSITIVE_U64] [--history-max-total-bytes POSITIVE_U64]
    [--out-dir ABSOLUTE_EXTERNAL_NEW_PATH]
    [--trace-mode off|record] [--trace-out ABSOLUTE_EXTERNAL_NEW_PATH]
    Default tier: small (16 files). medium=256, large=4096, xlarge=32768.
    scale-supported-v1: timeout=600s, history pair=1GiB/total=2GiB, ingest text=128MiB.
    Publications above 64MiB use 1MiB staged parts; sealed body cap=512MiB, process memory ceiling=4GiB.
    Explicit timeout/history overrides remain diagnostic inputs.
    A total-history override requires an explicit pair-history override.
    Disk observation defaults to boundary-only: start/end disk census outside timed work.
    periodic adds background disk resource sampling during timed work and may affect latency.
    --help, -h  Print this usage without running the rail.";

struct CliArgs {
    out_dir: PathBuf,
    seed: u64,
    tiers: Vec<ScaleTier>,
    fresh_output: bool,
    client_timeout_ms: Option<u64>,
    disk_observation: DiskObservationMode,
    history_max_bytes: Option<u64>,
    history_max_total_bytes: Option<u64>,
    trace_out: Option<PathBuf>,
}

struct RunOutcome<T> {
    benchmark_result: AnyResult<T>,
    trace_status: Option<String>,
}

fn parse_client_timeout_ms(raw: &str) -> AnyResult<u64> {
    let parsed = raw.parse::<u64>()?;
    if !(1..=600_000).contains(&parsed) {
        anyhow::bail!("--client-timeout-ms must be in 1..=600000");
    }
    Ok(parsed)
}

fn parse_history_max_bytes(raw: &str) -> AnyResult<u64> {
    let parsed = raw.parse::<u64>()?;
    if parsed == 0 {
        anyhow::bail!("history byte bound must be positive");
    }
    Ok(parsed)
}

fn parse_disk_observation(raw: &str) -> AnyResult<DiskObservationMode> {
    match raw {
        "boundary-only" => Ok(DiskObservationMode::BoundaryOnly),
        "periodic" => Ok(DiskObservationMode::Periodic),
        _ => anyhow::bail!("--disk-observation must be boundary-only or periodic"),
    }
}

fn parse_args() -> AnyResult<CliArgs> {
    parse_args_from(std::env::args().skip(1))
}

fn parse_args_from(args: impl IntoIterator<Item = String>) -> AnyResult<CliArgs> {
    let mut out_dir = PathBuf::from("artifacts/search-quality/scale/latest");
    let mut seed = DEFAULT_SEED;
    let mut tiers = vec![ScaleTier::Small];
    let mut out_dir_explicit = false;
    let mut client_timeout_ms = None;
    let mut disk_observation = DiskObservationMode::default();
    let mut history_max_bytes = None;
    let mut history_max_total_bytes = None;
    let mut trace_mode = "off";
    let mut trace_out = None;
    let mut args = args.into_iter();
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
            "--disk-observation" => {
                let raw = args
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--disk-observation requires a value"))?;
                disk_observation = parse_disk_observation(&raw)?;
            }
            "--history-max-bytes" => {
                let raw = args
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--history-max-bytes requires a value"))?;
                history_max_bytes = Some(parse_history_max_bytes(&raw)?);
            }
            "--history-max-total-bytes" => {
                let raw = args
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--history-max-total-bytes requires a value"))?;
                history_max_total_bytes = Some(parse_history_max_bytes(&raw)?);
            }
            "--trace-mode" => {
                let raw = args
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--trace-mode requires off or record"))?;
                trace_mode = match raw.as_str() {
                    "off" => "off",
                    "record" => "record",
                    _ => anyhow::bail!("--trace-mode must be off or record"),
                };
            }
            "--trace-out" => {
                trace_out =
                    Some(PathBuf::from(args.next().ok_or_else(|| {
                        anyhow::anyhow!("--trace-out requires a path")
                    })?));
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
    if !out_dir_explicit && tiers.as_slice() != [ScaleTier::Small] {
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
    if (trace_mode == "record") != trace_out.is_some() {
        anyhow::bail!("record mode requires --trace-out; off mode forbids it");
    }
    if let Some(path) = &trace_out {
        if !path.is_absolute() || path.exists() {
            anyhow::bail!("--trace-out must be an absolute new output path");
        }
        let parent = path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("--trace-out has no parent"))?;
        let existing_parent = if parent == out_dir {
            out_dir
                .parent()
                .ok_or_else(|| anyhow::anyhow!("--out-dir has no parent"))?
        } else {
            parent
        };
        let parent = std::fs::canonicalize(existing_parent)?;
        let checkout = std::fs::canonicalize(".")?;
        if parent.starts_with(checkout) {
            anyhow::bail!("--trace-out must be outside the checkout");
        }
    }
    let _history = ScaleRuntimeConfig {
        client_timeout: client_timeout_ms.map(Duration::from_millis),
        disk_observation,
        history_max_bytes,
        history_max_total_bytes,
    }
    .execution_json()?;
    Ok(CliArgs {
        out_dir,
        seed,
        tiers,
        fresh_output: out_dir_explicit,
        client_timeout_ms,
        disk_observation,
        history_max_bytes,
        history_max_total_bytes,
        trace_out,
    })
}

fn run(cli: &CliArgs) -> RunOutcome<Vec<TierMeasurement>> {
    let outcome = (|| -> AnyResult<RunOutcome<Vec<TierMeasurement>>> {
        // Establish provenance and the output root before creating the recorder.
        let git_head = GitHeadV1::resolve(Path::new("."))?;
        let host = HostV1::observe()?;
        if cli.fresh_output {
            if let Some(parent) = cli.out_dir.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::create_dir(&cli.out_dir)?;
        }
        let mut capture = cli
            .trace_out
            .as_ref()
            .map(|path| {
                DiagnosticCaptureV1::start(path.clone(), DiagnosticCaptureLimitsV1::default())
            })
            .transpose()?;
        let benchmark_result = run_with_capture(cli, capture.as_ref(), &git_head, &host);
        Ok(close_capture_preserving_benchmark(
            capture.as_mut(),
            benchmark_result,
        ))
    })();
    match outcome {
        Ok(outcome) => outcome,
        Err(error) => RunOutcome {
            benchmark_result: Err(error),
            trace_status: None,
        },
    }
}

fn close_capture_preserving_benchmark<T>(
    capture: Option<&mut DiagnosticCaptureV1>,
    benchmark_result: AnyResult<T>,
) -> RunOutcome<T> {
    let close = capture.map(|capture| capture.close(Duration::from_secs(5)));
    let trace_status = match close {
        None => None,
        Some(Ok(closed)) => Some(format!(
            "status={:?} events_written={} events_dropped={} events_unwritten={}",
            closed.status, closed.events_written, closed.events_dropped, closed.events_unwritten
        )),
        Some(Err(error)) => Some(format!("status=close_error reason={error}")),
    };
    RunOutcome {
        benchmark_result,
        trace_status,
    }
}

fn run_with_capture(
    cli: &CliArgs,
    capture: Option<&DiagnosticCaptureV1>,
    git_head: &GitHeadV1,
    host: &HostV1,
) -> AnyResult<Vec<TierMeasurement>> {
    // This lease remains live through the measurement and either artifact
    // path. Each runtime gets a separate child lease under one invocation root.
    let invocation_lease = capture.and_then(DiagnosticCaptureV1::lease);
    let invocation = invocation_lease
        .as_ref()
        .map(|lease| {
            lease
                .context()
                .enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(1))
        })
        .transpose()?;
    let artifact_context = invocation_lease
        .as_ref()
        .zip(invocation.as_ref())
        .map(|(lease, root)| lease.context().child_context(root));
    let result = (|| -> AnyResult<Vec<TierMeasurement>> {
        let mut measurements = Vec::with_capacity(cli.tiers.len());
        let runtime_config = ScaleRuntimeConfig {
            client_timeout: cli.client_timeout_ms.map(Duration::from_millis),
            disk_observation: cli.disk_observation,
            history_max_bytes: cli.history_max_bytes,
            history_max_total_bytes: cli.history_max_total_bytes,
        };
        let execution = runtime_config.execution_json()?;
        for tier in &cli.tiers {
            let runtime_lease = invocation
                .as_ref()
                .and_then(|root| invocation_lease.as_ref()?.child_of(root));
            match measure_tier_with_runtime_config_and_capture(
                *tier,
                cli.seed,
                runtime_config,
                runtime_lease,
            ) {
                Ok(measurement) => measurements.push(measurement),
                Err(error) => {
                    let binding = match source_binding_for_failure(*tier, cli.seed) {
                        Ok(binding) => binding,
                        Err(failure) => {
                            return Err(error.context(format!(
                                "refusal source binding also failed: {failure:#}"
                            )));
                        }
                    };
                    if let Err(failure) = trace_artifact_write(artifact_context.as_ref(), || {
                        write_refusal_artifact_with_context(
                            &binding,
                            &cli.out_dir,
                            git_head,
                            host,
                            &error,
                            "scale",
                            Some(&execution),
                        )
                    }) {
                        return Err(error
                            .context(format!("refusal artifact write also failed: {failure:#}")));
                    }
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
            trace_artifact_write(artifact_context.as_ref(), || {
                write_artifacts(measurement, &out_dir, (*git_head).clone(), (*host).clone())
            })?;
        }
        Ok(measurements)
    })();
    let outcome = DiagnosticTerminalV1::from_result(&result);
    let finished = invocation
        .map(|invocation| invocation.finish(outcome))
        .transpose();
    let measurements = result?;
    let _span_finished = finished?;
    Ok(measurements)
}

fn trace_artifact_write<T>(
    diagnostic: Option<&DiagnosticContextV1>,
    work: impl FnOnce() -> AnyResult<T>,
) -> AnyResult<T> {
    let span = diagnostic
        .map(|context| context.enter(DiagnosticStageV1::ArtifactWrite, DiagnosticLaneV1(1)))
        .transpose()?;
    let result = work();
    let outcome = DiagnosticTerminalV1::from_result(&result);
    let finished = span.map(|span| span.finish(outcome)).transpose();
    let artifact = result?;
    let _span_finished = finished?;
    Ok(artifact)
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
    let args: Vec<String> = std::env::args().skip(1).collect();
    if matches!(args.as_slice(), [arg] if matches!(arg.as_str(), "--help" | "-h")) {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    let cli = match parse_args() {
        Ok(cli) => cli,
        Err(err) => {
            eprintln!("scale_matrix: {err:#}");
            return ExitCode::FAILURE;
        }
    };
    let outcome = run(&cli);
    let measurements = match outcome.benchmark_result {
        Ok(measurements) => {
            if let Some(status) = outcome.trace_status {
                println!("trace_capture: {status}");
            }
            measurements
        }
        Err(err) => {
            if let Some(status) = outcome.trace_status {
                eprintln!("trace_capture: {status}");
            }
            eprintln!("scale_matrix: {err:#}");
            return ExitCode::FAILURE;
        }
    };
    for measurement in &measurements {
        println!(
            "scale[{}]: seed={} serving_owners=1 source_repos={} files={} build_ms={:.3} activation_ms={:.3} first_query_ms={:.3} warm_p50_ms={:.3} daemon_cold_open_ms={} adapter_open_ms={:.3} plan_ms={:.3} execute_ms={:.3} update_ms={:.3} noop_seal_ms={:.3} noop_activation_ms={:.3} reclaimed_bytes={} results={}",
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
            measurement.noop.seal_ms,
            measurement.noop.activation_ms,
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
                delete.delete_seal,
                delete.delete_activation,
                delete.same_process_reopen,
                delete.reopened_first_query,
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
    use super::{
        DiagnosticCaptureLimitsV1, DiagnosticCaptureV1, DiskObservationMode, format_cold_open_ms,
        parse_args_from, parse_client_timeout_ms, parse_history_max_bytes,
    };

    #[test]
    fn disk_observation_parser_defaults_and_refuses_invalid_inputs() {
        let default = parse_args_from(Vec::<String>::new()).map(|args| args.disk_observation);
        assert!(matches!(default, Ok(DiskObservationMode::BoundaryOnly)));

        let periodic = parse_args_from(["--disk-observation", "periodic"].map(str::to_owned))
            .map(|args| args.disk_observation);
        assert!(matches!(periodic, Ok(DiskObservationMode::Periodic)));

        let explicit_boundary =
            parse_args_from(["--disk-observation", "boundary-only"].map(str::to_owned))
                .map(|args| args.disk_observation);
        assert!(matches!(
            explicit_boundary,
            Ok(DiskObservationMode::BoundaryOnly)
        ));

        assert!(matches!(
            parse_args_from(["--disk-observation"].map(str::to_owned)),
            Err(error) if error.to_string() == "--disk-observation requires a value"
        ));
        assert!(matches!(
            parse_args_from(["--disk-observation", "sampled"].map(str::to_owned)),
            Err(error) if error.to_string() == "--disk-observation must be boundary-only or periodic"
        ));
    }

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

    #[test]
    fn history_budget_override_is_explicit_and_bounded() {
        assert_eq!(
            parse_history_max_bytes("268435456").expect("256 MiB"),
            268_435_456
        );
        assert_eq!(
            parse_history_max_bytes("536870912").expect("diagnostic pair bound"),
            536_870_912
        );
        for invalid in ["0", "nan", "-1"] {
            assert!(parse_history_max_bytes(invalid).is_err());
        }
    }

    #[test]
    fn trace_mode_requires_one_new_external_output() {
        let root = tempfile::tempdir().expect("external output parent");
        let output = root.path().join("new-output");
        let trace = output.join("causal-timeline.jsonl");
        let root_flag = output.to_string_lossy().into_owned();
        let trace_flag = trace.to_string_lossy().into_owned();
        assert!(
            parse_args_from([
                "--out-dir".to_owned(),
                root_flag.clone(),
                "--trace-mode".to_owned(),
                "record".to_owned(),
                "--trace-out".to_owned(),
                trace_flag.clone(),
            ])
            .is_ok()
        );
        assert!(
            parse_args_from([
                "--out-dir".to_owned(),
                root_flag.clone(),
                "--trace-mode".to_owned(),
                "off".to_owned(),
            ])
            .is_ok()
        );
        assert!(
            parse_args_from([
                "--out-dir".to_owned(),
                root_flag.clone(),
                "--trace-mode".to_owned(),
                "record".to_owned(),
            ])
            .is_err()
        );
        assert!(
            parse_args_from([
                "--out-dir".to_owned(),
                root_flag,
                "--trace-out".to_owned(),
                trace_flag,
            ])
            .is_err()
        );
    }

    #[test]
    fn trace_close_status_is_retained_when_workload_fails() {
        let root = tempfile::tempdir().expect("external trace output");
        let path = root.path().join("causal-timeline.jsonl");
        let mut capture = DiagnosticCaptureV1::start(path, DiagnosticCaptureLimitsV1::default())
            .expect("start capture");

        let outcome = super::close_capture_preserving_benchmark(
            Some(&mut capture),
            Err::<(), _>(anyhow::anyhow!("workload failure")),
        );

        assert!(
            matches!(outcome.benchmark_result, Err(error) if error.to_string() == "workload failure")
        );
        assert!(
            outcome
                .trace_status
                .as_deref()
                .is_some_and(|status| status.starts_with("status=Complete "))
        );
    }

    #[test]
    fn trace_close_error_does_not_revoke_successful_workload() {
        let root = tempfile::tempdir().expect("external trace output");
        let path = root.path().join("causal-timeline.jsonl");
        let mut capture = DiagnosticCaptureV1::start(path, DiagnosticCaptureLimitsV1::default())
            .expect("start capture");
        let lease = capture.lease().expect("active capture lease");

        let outcome = super::close_capture_preserving_benchmark(
            Some(&mut capture),
            Ok::<_, anyhow::Error>(()),
        );

        assert!(outcome.benchmark_result.is_ok());
        assert!(
            outcome
                .trace_status
                .as_deref()
                .is_some_and(|status| status.starts_with("status=close_error reason="))
        );
        drop(lease);
        drop(
            capture
                .close(std::time::Duration::from_secs(5))
                .expect("capture closes after lease release"),
        );
    }
}
