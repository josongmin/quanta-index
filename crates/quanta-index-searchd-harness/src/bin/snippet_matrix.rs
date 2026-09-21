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

/// The exact head of a clean worktree, or a refusal: a verdict artifact
/// that cannot say which source it judged is not written (QI-BB-010).
fn git_head() -> Result<String, quanta_index_searchd_harness::artifact::BenchProvenanceError> {
    quanta_index_searchd_harness::artifact::GitHeadV1::resolve(std::path::Path::new("."))
        .map(|head| head.as_str().to_string())
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
    let rev = match git_head() {
        Ok(head) => head,
        Err(err) => {
            eprintln!("snippet_matrix: {err}");
            return ExitCode::FAILURE;
        }
    };

    let report = match run_snippet_report() {
        Ok(report) => report,
        Err(err) => {
            eprintln!("snippet_matrix: rail run failed: {err:#}");
            return ExitCode::FAILURE;
        }
    };

    if let Err(err) = write_artifacts(&report, &out_dir, &rev) {
        eprintln!("snippet_matrix: failed to write artifacts under {}: {err:#}", out_dir.display());
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
