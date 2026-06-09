//! `relevance_matrix` — ranking-quality rail artifact producer + gate (J7Q-01A).
//!
//! Seeds the judged relevance fixture, runs each judged query against the live
//! ranker, scores the produced ordering (`MRR@10` / `NDCG@10` / `Recall@20`)
//! plus blocking ordering invariants, writes the canonical artifacts under
//! `artifacts/search-quality/relevance/latest/`, and exits non-zero on any
//! per-query or per-route failure. This is the authority behind
//! `just rust-verify-quality-relevance`.

use std::path::PathBuf;
use std::process::ExitCode;

use quanta_index_searchd_harness::relevance::report::{run_relevance_report, write_artifacts};

fn git_rev() -> String {
    if let Ok(rev) = std::env::var("DSL_BENCH_GIT_REV") {
        return rev;
    }
    "unknown".to_string()
}

/// Capture date stamped on the Sourcegraph overlap rows (J7Q-01B).
///
/// Injected via env so the recipe controls it and emission stays reproducible;
/// absent the env it reads `unprovisioned-capture-date` (honest: no external
/// capture happened).
fn capture_date() -> String {
    if let Ok(date) = std::env::var("QUANTA_QUALITY_CAPTURE_DATE") {
        return date;
    }
    "unprovisioned-capture-date".to_string()
}

fn parse_out_dir() -> PathBuf {
    let mut args = std::env::args().skip(1);
    match (args.next().as_deref(), args.next()) {
        (Some("--out-dir"), Some(path)) => PathBuf::from(path),
        _ => PathBuf::from("artifacts/search-quality/relevance/latest"),
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
    let capture_date = capture_date();

    let report = match run_relevance_report() {
        Ok(report) => report,
        Err(err) => {
            eprintln!("relevance_matrix: rail run failed: {err:#}");
            return ExitCode::FAILURE;
        }
    };

    if let Err(err) = write_artifacts(&report, &out_dir, &rev, &capture_date) {
        eprintln!(
            "relevance_matrix: failed to write artifacts under {}: {err:#}",
            out_dir.display()
        );
        return ExitCode::FAILURE;
    }

    for route in &report.routes {
        println!(
            "relevance[{}]: queries={} MRR@10={:.4} NDCG@10={:.4} Recall@20={:.4} passed={}",
            route.route.as_str(),
            route.query_count,
            route.mean_mrr_at_10,
            route.mean_ndcg_at_10,
            route.mean_recall_at_20,
            route.passed,
        );
    }
    for query in &report.queries {
        if !query.passed() {
            eprintln!("relevance FAIL [{}] ({})", query.id, query.intent);
            for failure in &query.failures {
                eprintln!("    - {failure}");
            }
            eprintln!("    produced order: {:?}", query.produced_order);
        }
    }

    if report.passed {
        println!("relevance rail green");
        ExitCode::SUCCESS
    } else {
        eprintln!("relevance rail RED: one or more route/query gates failed");
        ExitCode::FAILURE
    }
}
