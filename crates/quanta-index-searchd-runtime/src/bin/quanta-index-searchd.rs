//! The release daemon process (SEP-21 P08 / S21-09).
//!
//! Exit codes are fixed policy: `0` clean operator shutdown; `70`
//! required-child death, startup rollback, or hard-deadline escalation;
//! `128 + signum` for the signal that latched an immediate abort. Boot
//! failures (config, lease, adapter construction) are startup failures
//! and exit `70` too.

use std::process::ExitCode;

use quanta_index_searchd::SearchdCommand;
use quanta_index_searchd_runtime::run_supervised;

/// One operator-facing boot/exit notice, on stderr: the process entry
/// is the one place these are written; the runtime keeps them as data.
#[expect(
    clippy::print_stderr,
    reason = "the process entry is the one place operator-facing boot and exit notices are written"
)]
fn log(line: &str) {
    eprintln!("searchd: {line}");
}

fn main() -> ExitCode {
    let command = match SearchdCommand::from_env() {
        Ok(command) => command,
        Err(error) => {
            log(&format!("boot failed: {error}"));
            return ExitCode::from(70);
        }
    };
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
