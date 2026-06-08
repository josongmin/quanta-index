//! `snippet_matrix` — snippet-quality rail artifact producer + gate (J7Q-02).
//!
//! Seeds the snippet fixture (phrase / regex / multi-hit / long-line), runs each
//! judged query against the live ranker, grades the produced
//! `LexicalCandidate.snippet` against the hit-centered-window + bounded-length +
//! deterministic-truncation contract (NOT mere substring presence), writes the
//! canonical artifacts under `artifacts/search-quality/snippet/latest/`, and
//! exits non-zero on any per-query gate failure. The snippet is graded
//! as-emitted: the rail never post-processes the engine output to force a pass,
//! so an unwindowed/untruncated long-line snippet trips the gate honestly.
//! Authority behind `just rust-verify-quality-snippet`.

use std::path::PathBuf;
use std::process::ExitCode;

use quanta_index_searchd_harness::snippet::{run_snippet_report, write_artifacts};

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
        _ => PathBuf::from("artifacts/search-quality/snippet/latest"),
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

    let report = match run_snippet_report() {
        Ok(report) => report,
        Err(err) => {
            eprintln!("snippet_matrix: rail run failed: {err:#}");
            return ExitCode::FAILURE;
        }
    };

    if let Err(err) = write_artifacts(&report, &out_dir, &rev) {
        eprintln!(
            "snippet_matrix: failed to write artifacts under {}: {err:#}",
            out_dir.display()
        );
        return ExitCode::FAILURE;
    }

    for score in &report.scores {
        let (window_len, multi_hit) = score
            .metrics
            .as_ref()
            .map_or((0, 0), |m| (m.window_len, m.multi_hit_count));
        println!(
            "snippet[{}] intent={} window_len={} multi_hit={} passed={}",
            score.id,
            score.intent.as_str(),
            window_len,
            multi_hit,
            score.passed(),
        );
        if !score.passed() {
            for failure in &score.failures {
                eprintln!("    - {failure}");
            }
        }
    }

    if report.passed {
        println!("snippet rail green");
        ExitCode::SUCCESS
    } else {
        eprintln!("snippet rail RED: one or more snippet gates failed");
        ExitCode::FAILURE
    }
}
