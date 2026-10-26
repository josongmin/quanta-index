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

use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use anyhow::{Context as _, Result as AnyResult};
use quanta_index_core::{
    DiagnosticContextV1, DiagnosticLaneV1, DiagnosticRootKindV1, DiagnosticStageV1,
    DiagnosticTerminalV1,
};
use quanta_index_searchd::{DiagnosticCaptureLimitsV1, DiagnosticCaptureV1};
use quanta_index_searchd_harness::artifact::{GitHeadV1, HostV1};
use quanta_index_searchd_harness::scale::{
    DiskObservationMode, NativeIndexMeasurement, ScaleRuntimeConfig, ScaleTier, TierMeasurement,
    measure_index_native_tier, measure_tier_with_runtime_config_and_capture,
    native_source_binding_for_failure, source_binding_for_failure, write_artifacts,
    write_index_native_artifacts, write_refusal_artifact_with_context,
};

/// Deterministic default seed so the rail is reproducible run-to-run unless an
/// operator overrides it via `--seed`.
const DEFAULT_SEED: u64 = 0x5161_5343_414c_4531;

const USAGE: &str = "Usage: scale_matrix [--tier small|medium|large|xlarge | --all-tiers]
    [--backend legacy|index-native] [--native-daemon-bin ABSOLUTE_EXECUTABLE]
    [--seed U64] [--client-timeout-ms 1..=600000]
    [--disk-observation boundary-only|periodic]
    [--history-max-bytes POSITIVE_U64] [--history-max-total-bytes POSITIVE_U64]
    [--out-dir ABSOLUTE_EXTERNAL_NEW_PATH]
    [--trace-mode off|record] [--trace-out ABSOLUTE_EXTERNAL_NEW_PATH]
    [--trace-max-output-bytes 6144..=1073741824]
    Default tier: small (16 files). medium=256, large=4096, xlarge=32768.
    scale-supported-v1: timeout=600s, history pair=1GiB/total=2GiB, ingest text=128MiB.
    Publications above 64MiB use 1MiB staged parts; sealed body cap=512MiB, process memory ceiling=4GiB.
    Explicit timeout/history overrides remain diagnostic inputs.
    A total-history override requires an explicit pair-history override.
    Disk observation defaults to boundary-only: start/end disk census outside timed work.
    periodic adds background disk resource sampling during timed work and may affect latency.
    Index-native: QUANTA_INDEX_NATIVE_WORK_LIMITS_JSON='{\"reads\":U64,\"writes\":U64,\"bytes\":U64,\"mutations\":U64}'
    is optional and requires exactly four positive unsigned integer fields. These fund cumulative
    logical work, not process RSS, allocated disk, free space, or physical resource reserves.
    The fixed native profile rejects large/xlarge before daemon execution; explicit funding is not a success guarantee.
    QUANTA_INDEX_CAUSAL_PROFILE_V1=1 is forbidden: legacy stderr profiling would contaminate Off/Record comparisons.
    --help, -h  Print this usage without running the rail.";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ScaleBackend {
    Legacy,
    IndexNative,
}

enum ScaleResult {
    Legacy(Box<TierMeasurement>),
    IndexNative(Box<NativeIndexMeasurement>),
}

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
    trace_max_output_bytes: u64,
    backend: ScaleBackend,
    native_daemon_bin: Option<PathBuf>,
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
    let legacy_profile_enabled = std::env::var_os("QUANTA_INDEX_CAUSAL_PROFILE_V1")
        .is_some_and(|value| value == std::ffi::OsStr::new("1"));
    parse_args_from(std::env::args().skip(1), legacy_profile_enabled)
}

fn parse_args_from(
    args: impl IntoIterator<Item = String>,
    legacy_profile_enabled: bool,
) -> AnyResult<CliArgs> {
    if legacy_profile_enabled {
        anyhow::bail!(
            "unset QUANTA_INDEX_CAUSAL_PROFILE_V1: scale Off/Record modes exclude legacy stderr profiling"
        );
    }
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
    let mut trace_max_output_bytes = None;
    let mut backend = ScaleBackend::Legacy;
    let mut native_daemon_bin = None;
    let mut args = args.into_iter();
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--backend" => {
                backend = match args.next().as_deref() {
                    Some("legacy") => ScaleBackend::Legacy,
                    Some("index-native") => ScaleBackend::IndexNative,
                    _ => anyhow::bail!("--backend must be legacy or index-native"),
                };
            }
            "--native-daemon-bin" => {
                native_daemon_bin =
                    Some(PathBuf::from(args.next().ok_or_else(|| {
                        anyhow::anyhow!("--native-daemon-bin requires a path")
                    })?));
            }
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
            "--trace-max-output-bytes" => {
                let raw = args
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--trace-max-output-bytes requires a value"))?;
                let bytes = raw
                    .parse::<u64>()
                    .context("--trace-max-output-bytes must be a positive u64")?;
                let _validated_limits = DiagnosticCaptureLimitsV1::default()
                    .with_max_output_bytes(bytes)
                    .context("--trace-max-output-bytes is outside capture limits")?;
                if trace_max_output_bytes.replace(bytes).is_some() {
                    anyhow::bail!("--trace-max-output-bytes may be specified only once");
                }
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
    if trace_max_output_bytes.is_some() && trace_out.is_none() {
        anyhow::bail!("--trace-max-output-bytes requires --trace-out and Record mode");
    }
    match backend {
        ScaleBackend::Legacy if native_daemon_bin.is_some() => {
            anyhow::bail!("--native-daemon-bin requires --backend index-native")
        }
        ScaleBackend::IndexNative => {
            let binary = native_daemon_bin.as_ref().ok_or_else(|| {
                anyhow::anyhow!("--backend index-native requires --native-daemon-bin")
            })?;
            if !binary.is_absolute()
                || !binary.is_file()
                || std::fs::metadata(binary)?.permissions().mode() & 0o111 == 0
            {
                anyhow::bail!("--native-daemon-bin must be an absolute existing executable");
            }
            if disk_observation != DiskObservationMode::BoundaryOnly
                || history_max_bytes.is_some()
                || history_max_total_bytes.is_some()
                || client_timeout_ms.is_some()
            {
                anyhow::bail!(
                    "index-native backend does not accept legacy runtime policy overrides"
                );
            }
        }
        ScaleBackend::Legacy => {}
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
        trace_max_output_bytes: trace_max_output_bytes
            .unwrap_or_else(|| DiagnosticCaptureLimitsV1::default().max_output_bytes),
        backend,
        native_daemon_bin,
    })
}

fn run(cli: &CliArgs) -> RunOutcome<Vec<ScaleResult>> {
    let outcome = (|| -> AnyResult<RunOutcome<Vec<ScaleResult>>> {
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
                DiagnosticCaptureV1::start(
                    path.clone(),
                    DiagnosticCaptureLimitsV1::default()
                        .with_max_output_bytes(cli.trace_max_output_bytes)?,
                )
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

// The parent capture path is already absolute, external and exclusive.
// Retain child streams beside it even when Small uses the default summary path.
fn native_child_capture_directory(trace_out: &Path, tier: ScaleTier) -> PathBuf {
    let mut child = trace_out.as_os_str().to_os_string();
    child.push(format!(".native-{}-children", tier.as_str()));
    PathBuf::from(child)
}

fn run_with_capture(
    cli: &CliArgs,
    capture: Option<&DiagnosticCaptureV1>,
    git_head: &GitHeadV1,
    host: &HostV1,
) -> AnyResult<Vec<ScaleResult>> {
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
    let result = (|| -> AnyResult<Vec<ScaleResult>> {
        let mut measurements = Vec::with_capacity(cli.tiers.len());
        let runtime_config = ScaleRuntimeConfig {
            client_timeout: cli.client_timeout_ms.map(Duration::from_millis),
            disk_observation: cli.disk_observation,
            history_max_bytes: cli.history_max_bytes,
            history_max_total_bytes: cli.history_max_total_bytes,
        };
        let execution = match cli.backend {
            ScaleBackend::Legacy => runtime_config.execution_json()?,
            ScaleBackend::IndexNative => serde_json::json!({
                "backend":"index-native-configured-daemon-uds",
                "scope":"index-only-source-event-to-ACK-to-searchability",
                "trace_scope":"harness phase spans plus independent child daemon captures; clocks are not aligned",
                "daemon_bin":cli.native_daemon_bin,
                "mixed_load_requested":std::env::var_os("QUANTA_INDEX_NATIVE_MIXED_LOAD_V1").is_some(),
            }),
        };
        let refuse = |tier: ScaleTier, error: anyhow::Error| -> anyhow::Error {
            let binding = match match cli.backend {
                ScaleBackend::Legacy => source_binding_for_failure(tier, cli.seed),
                ScaleBackend::IndexNative => native_source_binding_for_failure(tier, cli.seed),
            } {
                Ok(binding) => binding,
                Err(failure) => {
                    return error
                        .context(format!("refusal source binding also failed: {failure:#}"));
                }
            };
            if let Err(failure) = trace_artifact_write(artifact_context.as_ref(), || {
                write_refusal_artifact_with_context(
                    &binding,
                    &cli.out_dir,
                    git_head,
                    host,
                    &error,
                    match cli.backend {
                        ScaleBackend::Legacy => "scale",
                        ScaleBackend::IndexNative => "scale-index-native",
                    },
                    Some(&execution),
                )
            }) {
                return error.context(format!("refusal artifact write also failed: {failure:#}"));
            }
            error
        };
        for tier in &cli.tiers {
            let child_capture_directory = cli
                .trace_out
                .as_deref()
                .map(|path| native_child_capture_directory(path, *tier));
            let runtime_lease = invocation
                .as_ref()
                .and_then(|root| invocation_lease.as_ref()?.child_of(root));
            let measured = match cli.backend {
                ScaleBackend::Legacy => measure_tier_with_runtime_config_and_capture(
                    *tier,
                    cli.seed,
                    runtime_config,
                    runtime_lease,
                )
                .map(|measurement| ScaleResult::Legacy(Box::new(measurement))),
                ScaleBackend::IndexNative => measure_index_native_tier(
                    *tier,
                    cli.seed,
                    cli.native_daemon_bin
                        .as_deref()
                        .ok_or_else(|| anyhow::anyhow!("native daemon binary missing"))?,
                    runtime_lease,
                    child_capture_directory.as_deref(),
                    cli.trace_max_output_bytes,
                )
                .map(|measurement| ScaleResult::IndexNative(Box::new(measurement))),
            };
            match measured {
                Ok(measurement) => measurements.push(measurement),
                Err(error) => return Err(refuse(*tier, error)),
            }
        }
        // An all-tier run does not write earlier tier artifacts if a later tier
        // refuses source or wire admission.
        for measurement in &measurements {
            let tier = match measurement {
                ScaleResult::Legacy(measurement) => measurement.tier,
                ScaleResult::IndexNative(measurement) => measurement.tier,
            };
            let out_dir = if cli.tiers.len() == 1 {
                cli.out_dir.clone()
            } else {
                cli.out_dir.join(tier.as_str())
            };
            let written = trace_artifact_write(artifact_context.as_ref(), || match measurement {
                ScaleResult::Legacy(measurement) => {
                    write_artifacts(measurement, &out_dir, (*git_head).clone(), (*host).clone())
                }
                ScaleResult::IndexNative(measurement) => write_index_native_artifacts(
                    measurement,
                    &out_dir,
                    (*git_head).clone(),
                    (*host).clone(),
                ),
            });
            if let Err(error) = written {
                let error = match measurement {
                    ScaleResult::Legacy(_) => error,
                    ScaleResult::IndexNative(measurement) => {
                        measurement.with_failure_capture_paths(error)
                    }
                };
                return Err(refuse(tier, error));
            }
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
        if let ScaleResult::IndexNative(native) = measurement {
            println!(
                "scale-index-native[{}]: seed={} files={} source_bytes={} full_event_ack_ms={:.3} first_search_ms={:.3} total_ms={:.3} scope=index-only",
                native.tier.as_str(),
                native.seed,
                native.file_count,
                native.source_bytes,
                native
                    .phases
                    .get("full_event_ack")
                    .map_or(0.0, |phase| phase.wall_ms),
                native.first_query_ms,
                native.total_ms,
            );
            continue;
        }
        let ScaleResult::Legacy(measurement) = measurement else {
            continue;
        };
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
    use std::os::unix::fs::PermissionsExt as _;

    use super::{
        DiagnosticCaptureLimitsV1, DiagnosticCaptureV1, DiskObservationMode, ScaleBackend,
        format_cold_open_ms, parse_args_from, parse_client_timeout_ms, parse_history_max_bytes,
    };

    #[test]
    fn native_backend_requires_explicit_executable_and_refuses_legacy_policy() {
        let temp = tempfile::tempdir().expect("native test root");
        let executable = temp.path().join("searchd");
        std::fs::write(&executable, b"#!/bin/sh\nexit 0\n").expect("write fixture executable");
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700))
            .expect("mark fixture executable");
        let args = [
            "--backend".to_owned(),
            "index-native".to_owned(),
            "--native-daemon-bin".to_owned(),
            executable.to_string_lossy().into_owned(),
        ];
        let parsed = parse_args_from(args.clone(), false).expect("explicit Native backend");
        assert_eq!(parsed.backend, ScaleBackend::IndexNative);
        assert!(parse_args_from(["--backend", "index-native"].map(str::to_owned), false).is_err());
        assert!(
            parse_args_from(
                args.into_iter()
                    .chain(["--history-max-bytes".to_owned(), "268435456".to_owned()]),
                false
            )
            .is_err()
        );
    }

    #[test]
    fn native_record_default_small_keeps_child_captures_external_without_output_override()
    -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        let binary = root.path().join("searchd");
        std::fs::write(&binary, b"#!/bin/sh\nexit 0\n")?;
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700))?;
        let trace = root.path().join("record.jsonl");
        let cli = parse_args_from(
            [
                "--backend".to_owned(),
                "index-native".to_owned(),
                "--native-daemon-bin".to_owned(),
                binary.to_string_lossy().into_owned(),
                "--trace-mode".to_owned(),
                "record".to_owned(),
                "--trace-out".to_owned(),
                trace.to_string_lossy().into_owned(),
            ],
            false,
        )?;
        let selected = cli
            .trace_out
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("Small parent capture path absent"))?;
        let small = super::native_child_capture_directory(selected, super::ScaleTier::Small);
        anyhow::ensure!(small.is_absolute() && small.parent() == trace.parent());
        anyhow::ensure!(!small.exists(), "path selection created trace output");
        let large = super::native_child_capture_directory(selected, super::ScaleTier::Large);
        anyhow::ensure!(large != small, "tiers share child captures");
        let off_cli = parse_args_from(
            [
                "--backend".to_owned(),
                "index-native".to_owned(),
                "--native-daemon-bin".to_owned(),
                binary.to_string_lossy().into_owned(),
            ],
            false,
        )?;
        anyhow::ensure!(
            off_cli
                .trace_out
                .as_deref()
                .map(|path| super::native_child_capture_directory(path, super::ScaleTier::Small))
                .is_none(),
            "Off mode selected a child capture directory"
        );
        Ok(())
    }

    #[test]
    fn disk_observation_parser_defaults_and_refuses_invalid_inputs() {
        let default =
            parse_args_from(Vec::<String>::new(), false).map(|args| args.disk_observation);
        assert!(matches!(default, Ok(DiskObservationMode::BoundaryOnly)));

        let periodic =
            parse_args_from(["--disk-observation", "periodic"].map(str::to_owned), false)
                .map(|args| args.disk_observation);
        assert!(matches!(periodic, Ok(DiskObservationMode::Periodic)));

        let explicit_boundary = parse_args_from(
            ["--disk-observation", "boundary-only"].map(str::to_owned),
            false,
        )
        .map(|args| args.disk_observation);
        assert!(matches!(
            explicit_boundary,
            Ok(DiskObservationMode::BoundaryOnly)
        ));

        assert!(matches!(
            parse_args_from(["--disk-observation"].map(str::to_owned), false),
            Err(error) if error.to_string() == "--disk-observation requires a value"
        ));
        assert!(matches!(
            parse_args_from(["--disk-observation", "sampled"].map(str::to_owned), false),
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
            parse_args_from(
                [
                    "--out-dir".to_owned(),
                    root_flag.clone(),
                    "--trace-mode".to_owned(),
                    "record".to_owned(),
                    "--trace-out".to_owned(),
                    trace_flag.clone(),
                ],
                false
            )
            .is_ok()
        );
        assert!(
            parse_args_from(
                [
                    "--out-dir".to_owned(),
                    root_flag.clone(),
                    "--trace-mode".to_owned(),
                    "off".to_owned(),
                ],
                false
            )
            .is_ok()
        );
        assert!(
            parse_args_from(
                [
                    "--out-dir".to_owned(),
                    root_flag.clone(),
                    "--trace-mode".to_owned(),
                    "record".to_owned(),
                ],
                false
            )
            .is_err()
        );
        assert!(
            parse_args_from(
                [
                    "--out-dir".to_owned(),
                    root_flag,
                    "--trace-out".to_owned(),
                    trace_flag,
                ],
                false
            )
            .is_err()
        );
    }

    #[test]
    fn trace_output_limit_is_explicit_and_bounded() {
        let root = tempfile::tempdir().expect("external output parent");
        let output = root.path().join("new-output");
        let trace = output.join("causal-timeline.jsonl");
        let base = [
            "--out-dir".to_owned(),
            output.to_string_lossy().into_owned(),
            "--trace-mode".to_owned(),
            "record".to_owned(),
            "--trace-out".to_owned(),
            trace.to_string_lossy().into_owned(),
        ];
        let default = parse_args_from(base.clone(), false).expect("default Record cap");
        assert_eq!(default.trace_max_output_bytes, 67_108_864);
        let selected = parse_args_from(
            base.clone().into_iter().chain([
                "--trace-max-output-bytes".to_owned(),
                "1073741824".to_owned(),
            ]),
            false,
        )
        .expect("explicit maximum Record cap");
        assert_eq!(selected.trace_max_output_bytes, 1_073_741_824);
        for invalid in [
            "0",
            "6143",
            "1073741825",
            "-1",
            "true",
            "18446744073709551616",
        ] {
            assert!(
                parse_args_from(
                    base.clone()
                        .into_iter()
                        .chain(["--trace-max-output-bytes".to_owned(), invalid.to_owned(),]),
                    false,
                )
                .is_err()
            );
        }
        assert!(
            parse_args_from(
                ["--trace-max-output-bytes", "6144"].map(str::to_owned),
                false,
            )
            .is_err()
        );
        assert!(
            parse_args_from(
                base.into_iter().chain([
                    "--trace-max-output-bytes".to_owned(),
                    "6144".to_owned(),
                    "--trace-max-output-bytes".to_owned(),
                    "6144".to_owned(),
                ]),
                false,
            )
            .is_err()
        );
    }

    #[test]
    fn legacy_stderr_profiling_cannot_contaminate_either_trace_mode() {
        let root = tempfile::tempdir().expect("external output parent");
        for mode in ["off", "record"] {
            let output = root.path().join(mode);
            let trace = root.path().join(format!("{mode}.jsonl"));
            let mut args = vec![
                "--out-dir".to_owned(),
                output.to_string_lossy().into_owned(),
                "--trace-mode".to_owned(),
                mode.to_owned(),
            ];
            if mode == "record" {
                args.extend([
                    "--trace-out".to_owned(),
                    trace.to_string_lossy().into_owned(),
                ]);
            }
            assert!(parse_args_from(args.clone(), false).is_ok());
            assert!(matches!(
                parse_args_from(args, true),
                Err(error) if error.to_string().contains("unset QUANTA_INDEX_CAUSAL_PROFILE_V1")
            ));
            assert!(!output.exists());
            assert!(!trace.exists());
        }
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
