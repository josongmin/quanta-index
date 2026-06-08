//! `ambiguity_matrix` — repairability rail artifact producer + gate (J7Q-06).
//!
//! Snapshots the typed repair payloads the search-plane wire boundary emits for
//! every bridge error code, enforces the repairability invariants (repairable
//! codes carry non-empty alternatives + a docs anchor; the internal code carries
//! none; classes stay distinct families; payloads round-trip the wire codec),
//! writes the canonical artifacts under
//! `artifacts/search-quality/ambiguity/latest/`, and exits non-zero on any
//! invariant failure. Authority behind `just rust-verify-quality-ambiguity`.

use std::path::PathBuf;
use std::process::ExitCode;

use quanta_index_searchd_harness::ambiguity::{run_ambiguity_report, write_artifacts};

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
        _ => PathBuf::from("artifacts/search-quality/ambiguity/latest"),
    }
}

#[expect(
    clippy::print_stderr,
    clippy::print_stdout,
    reason = "rail binary reports rail status on stdout/stderr by design"
)]
fn main() -> ExitCode {
    let out_dir = parse_out_dir();
    let report = run_ambiguity_report();

    if let Err(err) = write_artifacts(&report, &out_dir, &git_rev()) {
        eprintln!(
            "ambiguity_matrix: failed to write artifacts under {}: {err:#}",
            out_dir.display()
        );
        return ExitCode::FAILURE;
    }

    for audit in &report.audits {
        let class = audit
            .repair
            .as_ref()
            .map_or("none", |r| r.class.as_code_str());
        println!(
            "ambiguity[{}]: repairable={} class={} passed={}",
            audit.code,
            audit.expected_repairable,
            class,
            audit.passed(),
        );
        for failure in &audit.failures {
            eprintln!("    - {failure}");
        }
    }

    if report.passed {
        println!("repairability rail green");
        ExitCode::SUCCESS
    } else {
        eprintln!("repairability rail RED: one or more invariants failed");
        ExitCode::FAILURE
    }
}
