//! Cold DSL matrix entry point for Native file samples and the Legacy gate.

use quanta_index_searchd_harness::artifact::BenchMode;
use quanta_index_searchd_harness::native_index::{
    NativeDslTiming, assemble_native_dsl_artifact, measure_cold_native_dsl_history,
    measure_native_dsl_text, native_dsl_scenario_roster,
};
use std::path::Path;
use std::process::ExitCode;

#[cfg(feature = "legacy-bench")]
#[path = "dsl_cold_matrix/legacy.rs"]
mod legacy;

/// Print one line of machine-readable output to stdout.
///
/// This binary's contract is to emit exactly one JSON document per run, so
/// stdout printing is the intended interface rather than incidental logging.
#[expect(
    clippy::print_stdout,
    reason = "the cold-matrix probe emits its JSON sample on stdout by design"
)]
fn emit_stdout(value: &serde_json::Value) {
    println!("{value}");
}

/// Print a diagnostic line to stderr.
///
/// Operator-facing errors go to stderr so the orchestrator can separate them
/// from the JSON sample on stdout.
#[expect(
    clippy::print_stderr,
    reason = "the cold-matrix probe reports operator errors on stderr by design"
)]
fn emit_stderr(message: &str) {
    eprintln!("{message}");
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.split_first() {
        Some((flag, [input_flag, input, out_flag, out]))
            if flag == "--assemble-native" && input_flag == "--input" && out_flag == "--out" =>
        {
            match assemble_native_dsl_artifact(Path::new(input), Path::new(out), BenchMode::Cold) {
                Ok(()) => ExitCode::SUCCESS,
                Err(err) => {
                    emit_stderr(&format!("dsl_cold_matrix: {err:#}"));
                    ExitCode::FAILURE
                }
            }
        }
        Some((flag, [])) if flag == "--list-native" => {
            emit_stdout(&native_dsl_scenario_roster());
            ExitCode::SUCCESS
        }
        Some((flag, [input, id])) if flag == "--native-text" || flag == "--native-history" => {
            let measured = if flag == "--native-text" {
                measure_native_dsl_text(Path::new(input), id, NativeDslTiming::PROCESS_COLD)
            } else {
                measure_cold_native_dsl_history(Path::new(input), id)
            };
            match measured {
                Ok(sample) => {
                    emit_stdout(&sample);
                    ExitCode::SUCCESS
                }
                Err(err) => {
                    emit_stderr(&format!("dsl_cold_matrix: {err:#}"));
                    ExitCode::FAILURE
                }
            }
        }
        #[cfg(feature = "legacy-bench")]
        _ => legacy::run(&args),
        #[cfg(not(feature = "legacy-bench"))]
        _ => {
            emit_stderr(
                "usage: dsl_cold_matrix (--list-native | --native-text <daemon> <scenario-id> | --native-history <cold-fixture-json> <scenario-id> | --assemble-native --input <matrix.json> --out <artifact.json>)",
            );
            ExitCode::FAILURE
        }
    }
}
