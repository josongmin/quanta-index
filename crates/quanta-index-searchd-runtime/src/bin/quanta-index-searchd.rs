//! The release daemon process (SEP-21 P08 / S21-09).
//!
//! Exit codes are fixed policy: `0` clean operator shutdown; `70`
//! required-child death, startup rollback, or hard-deadline escalation;
//! `128 + signum` for the signal that latched an immediate abort. Boot
//! failures (config, lease, adapter construction) are startup failures
//! and exit `70` too.
//!
//! The same process entry carries the offline state commands (SEP-21 P10 /
//! S21-11): `backup-state`, `restore-state` and
//! `verify-state` run the composition root's offline workflow and exit `0`
//! or `70`. An offline invocation never falls through to `serve`.

use std::process::ExitCode;

use quanta_index_searchd::SearchdCommand;
#[cfg(feature = "legacy-tantivy")]
use quanta_index_searchd_runtime::run_supervised;
#[cfg(feature = "legacy-tantivy")]
use quanta_index_searchd_runtime::state_migration::{
    render_offline_outcome_v1, run_offline_state_command_v1,
};

/// One operator-facing boot/exit notice, on stderr: the process entry
/// is the one place these are written; the runtime keeps them as data.
#[expect(
    clippy::print_stderr,
    reason = "the process entry is the one place operator-facing boot and exit notices are written"
)]
fn log(line: &str) {
    eprintln!("searchd: {line}");
}

/// One operator-facing offline notice, on stdout: the offline workflow's
/// receipt line, so a scripted cutover can read it.
#[expect(
    clippy::print_stdout,
    reason = "the process entry is the one place the offline workflow's receipt line is written"
)]
#[cfg(feature = "legacy-tantivy")]
fn report(line: &str) {
    println!("{line}");
}

fn run_native(
    command: &quanta_index_searchd::cli::NativeStateCommandV1,
    trace_path: Option<&std::path::Path>,
    trace_limits: quanta_index_searchd::DiagnosticCaptureLimitsV1,
) -> anyhow::Result<u8> {
    let Some(path) = trace_path else {
        return quanta_index_searchd_runtime::native_product::run_command(command);
    };
    let _inherited_umask = quanta_index_searchd::app::harden_umask();
    let mut capture =
        quanta_index_searchd::DiagnosticCaptureV1::start(path.to_path_buf(), trace_limits)?;
    let lease = capture
        .lease()
        .ok_or_else(|| anyhow::anyhow!("Native capture refused producer lease"))?;
    let result =
        quanta_index_searchd_runtime::native_product::run_command_with_capture(command, lease);
    let closed = capture.close(quanta_index_searchd::HARD_DRAIN_DEADLINE);
    match (result, closed) {
        (result, Ok(_closed)) => result,
        (Ok(0), Err(error)) => Err(anyhow::anyhow!("Native capture close failed: {error}")),
        (Ok(code), Err(error)) => {
            log(&format!(
                "Native capture incomplete after exit {code}: {error}"
            ));
            Ok(code)
        }
        (Err(primary), Err(error)) => {
            Err(primary.context(format!("Native capture close also failed: {error}")))
        }
    }
}

fn main() -> ExitCode {
    let command = match SearchdCommand::from_env() {
        Ok(command) => command,
        Err(error) => {
            log(&format!("boot failed: {error}"));
            return ExitCode::from(70);
        }
    };
    #[cfg(unix)]
    if let Some(export) = command.history_export_command() {
        let request =
            quanta_index_core::RequestBudgetV1::for_duration(std::time::Duration::from_secs(600));
        return match quanta_index_searchd_runtime::native_product::history_checkpoint_export::run(
            export, request,
        ) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                log(&format!("History checkpoint export failed: {error}"));
                ExitCode::from(70)
            }
        };
    }
    #[cfg(unix)]
    if let Some(recovery) = command.recovery_command() {
        let request =
            quanta_index_core::RequestBudgetV1::for_duration(std::time::Duration::from_secs(60));
        return match quanta_index_searchd_runtime::native_recovery::run(recovery, request) {
            Ok(code) => ExitCode::from(code),
            Err(error) => {
                log(&format!("native recovery failed: {error:#}"));
                ExitCode::from(70)
            }
        };
    }
    #[cfg(not(feature = "legacy-tantivy"))]
    if command.offline_operation().is_some() {
        log("legacy state command unavailable in Native-only build");
        return ExitCode::from(70);
    }
    #[cfg(feature = "legacy-tantivy")]
    if let Some(offline) = command.offline_operation().cloned() {
        let outcome = quanta_index_searchd::app::config::SearchdConfig::from_env_with_state_root(
            offline.source_root.clone(),
        )
        .and_then(|config| {
            run_offline_state_command_v1(
                &offline,
                &config,
                &quanta_index_core::RequestBudgetV1::unbounded(),
            )
        });
        return match outcome {
            Ok(outcome) => {
                report(&render_offline_outcome_v1(&offline, &outcome));
                ExitCode::SUCCESS
            }
            Err(error) => {
                log(&format!(
                    "{} failed: {error}",
                    offline.operation.command_name()
                ));
                ExitCode::from(70)
            }
        };
    }
    if let Some(native) = command.native_command() {
        log("native_boot_stage=config_preflight");
        return match run_native(
            native,
            command
                .diagnostic_trace_path()
                .map(std::path::PathBuf::as_path),
            command.diagnostic_trace_limits(),
        ) {
            Ok(code) => ExitCode::from(code),
            Err(error) => {
                log(&format!("native boot or service failed: {error:#}"));
                ExitCode::from(70)
            }
        };
    }
    #[cfg(not(feature = "legacy-tantivy"))]
    {
        log("legacy serve unavailable in Native-only build");
        ExitCode::from(70)
    }
    #[cfg(feature = "legacy-tantivy")]
    match run_supervised(command) {
        Ok(outcome) => {
            let code = outcome.exit_code();
            if code != 0 {
                log(&format!("supervised exit {code}: {outcome:?}"));
            }
            u8::try_from(code).map_or_else(|_out_of_range| ExitCode::from(70), ExitCode::from)
        }
        Err(error) => {
            log(&format!("boot failed: {error}"));
            ExitCode::from(70)
        }
    }
}
