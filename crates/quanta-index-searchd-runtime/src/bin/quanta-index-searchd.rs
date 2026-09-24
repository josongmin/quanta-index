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
use quanta_index_searchd_runtime::run_supervised;
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
fn report(line: &str) {
    println!("{line}");
}

fn main() -> ExitCode {
    let command = match SearchdCommand::from_env() {
        Ok(command) => command,
        Err(error) => {
            log(&format!("boot failed: {error}"));
            return ExitCode::from(70);
        }
    };
    if let Some(offline) = command.offline_operation().cloned() {
        return match run_offline_state_command_v1(&offline) {
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
