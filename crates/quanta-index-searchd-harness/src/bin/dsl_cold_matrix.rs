//! `dsl_cold_matrix` — cold-start single-query probe for the Layer-3 DSL
//! latency matrix (RFC-DSL-Benchmarking §3.2).
//!
//! Each invocation is **one fresh process = one true cold sample**. The
//! `tools/benchmark/run_dsl_cold_matrix.py` orchestrator drives this binary
//! `--scenario <id>` K times per scenario and aggregates the percentiles.
//!
//! The binary never aggregates and never fabricates a number: it boots, seeds
//! the scenario's fixture, seals, activates, times exactly one query, prints
//! one JSON sample, and exits. A query that the runtime rejects is reported with
//! its typed error code and a null result count, never a guessed latency.
//!
//! Subcommands:
//!   --list             print the scenario authority as a JSON array
//!   --scenario <id>    run one cold sample and print a JSON object

use std::process::ExitCode;
use std::time::Instant;

use quanta_index_searchd_harness::E2eRuntime;
use quanta_index_searchd_harness::artifact::BenchMode;
use quanta_index_searchd_harness::bench_support::{prepare_cold_runtime, run_scenario_query};
use quanta_index_searchd_harness::scenarios::{DslBenchScenario, SCENARIOS, scenario_by_id};

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

fn git_rev() -> String {
    if let Ok(rev) = std::env::var("DSL_BENCH_GIT_REV") {
        return rev;
    }
    "unknown".to_string()
}

fn print_list() {
    let rows: Vec<serde_json::Value> = SCENARIOS
        .iter()
        .map(|scenario| {
            serde_json::json!({
                "scenario_id": scenario.id,
                "route_family": scenario.route_family.as_str(),
                "syntax": scenario.syntax.as_str(),
                "expected_shape": scenario.expected_shape.as_str(),
            })
        })
        .collect();
    emit_stdout(&serde_json::Value::Array(rows));
}

fn run_scenario(scenario: &DslBenchScenario) -> ExitCode {
    let rev = git_rev();
    let mut runtime = match prepare_cold_runtime(scenario) {
        Ok(runtime) => runtime,
        Err(err) => {
            emit_stderr(&format!(
                "dsl_cold_matrix: prepare failed for {}: {err:#}",
                scenario.id
            ));
            return ExitCode::FAILURE;
        }
    };
    emit_stdout(&measured_sample(&mut runtime, scenario, &rev));
    ExitCode::SUCCESS
}

/// Time exactly one cold query and build its JSON sample.
fn measured_sample(
    runtime: &mut E2eRuntime,
    scenario: &DslBenchScenario,
    rev: &str,
) -> serde_json::Value {
    let started = Instant::now();
    let outcome = run_scenario_query(runtime, scenario);
    let first_query_ms = started.elapsed().as_secs_f64() * 1000.0;
    serde_json::json!({
        "scenario_id": scenario.id,
        "route_family": scenario.route_family.as_str(),
        "syntax": scenario.syntax.as_str(),
        "mode": BenchMode::Cold.as_str(),
        "result_shape": outcome.result_shape.as_str(),
        "first_query_ms": first_query_ms,
        "result_count": outcome.result_count,
        "typed_error_code": outcome.typed_error_code,
        "engine_touched": outcome.engine_touched,
        "early_stop_reason": outcome.early_stop_reason,
        "git_rev": rev,
    })
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.split_first() {
        Some((flag, rest)) if flag.as_str() == "--list" && rest.is_empty() => {
            print_list();
            ExitCode::SUCCESS
        }
        Some((flag, rest)) if flag.as_str() == "--scenario" => {
            let Some(id) = rest.first() else {
                emit_stderr("dsl_cold_matrix: --scenario requires an <id>");
                return ExitCode::FAILURE;
            };
            scenario_by_id(id).map_or_else(
                || {
                    emit_stderr(&format!("dsl_cold_matrix: unknown scenario id `{id}`"));
                    ExitCode::FAILURE
                },
                run_scenario,
            )
        }
        _ => {
            emit_stderr("usage: dsl_cold_matrix (--list | --scenario <id>)");
            ExitCode::FAILURE
        }
    }
}
