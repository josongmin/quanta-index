//! `ui_matrix` — UI/UX contract rail artifact producer + gate (J7Q-07).
//!
//! Seeds the UI fixtures (short hit + long-line hit), runs each probe against the
//! live ranker, and checks the typed `LexicalCandidate::snippet_hit_offset`
//! highlight anchor is present and points exactly at the matched needle. Writes
//! the canonical artifacts under `artifacts/search-quality/ui/latest/` and exits
//! non-zero on any per-probe anchor failure. Authority behind
//! `just rust-verify-quality-ui`.
//!
//! Fail-closed: a missing / out-of-range / wrong anchor trips the gate honestly;
//! the rail never fabricates an anchor to force a pass.

use std::path::PathBuf;
use std::process::ExitCode;

use quanta_index_searchd_harness::ui::{run_ui_report, write_artifacts};

fn git_rev() -> String {
    if let Ok(rev) = std::env::var("DSL_BENCH_GIT_REV") {
        return rev;
    }
    "unknown".to_string()
}

fn parse_out_dir() -> PathBuf {
    let mut args = std::env::args().skip(1);
    match (args.next().as_deref(), args.next()) {
        (Some("--out-dir"), Some(path)) => PathBuf::from(path),
        _ => PathBuf::from("artifacts/search-quality/ui/latest"),
    }
}

#[expect(
    clippy::print_stderr,
    clippy::print_stdout,
    reason = "rail binary reports rail status on stdout/stderr by design"
)]
fn main() -> ExitCode {
    let out_dir = parse_out_dir();
    let rev = git_rev();

    let report = match run_ui_report() {
        Ok(report) => report,
        Err(err) => {
            eprintln!("ui_matrix: rail run failed: {err:#}");
            return ExitCode::FAILURE;
        }
    };

    if let Err(err) = write_artifacts(&report, &out_dir, &rev) {
        eprintln!(
            "ui_matrix: failed to write artifacts under {}: {err:#}",
            out_dir.display()
        );
        return ExitCode::FAILURE;
    }

    for score in &report.scores {
        println!(
            "ui[{}] snippet_hit_offset={:?} passed={}",
            score.id,
            score.snippet_hit_offset,
            score.passed(),
        );
        if !score.passed() {
            for failure in &score.failures {
                eprintln!("    - {failure}");
            }
        }
    }

    if report.passed {
        println!(
            "ui rail green (typed snippet_hit_offset anchor points at the hit on every probe)"
        );
        ExitCode::SUCCESS
    } else {
        eprintln!("ui rail RED: a probe carried a missing or wrong UI highlight anchor");
        ExitCode::FAILURE
    }
}
