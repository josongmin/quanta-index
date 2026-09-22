//! `ops_matrix` — operator-ergonomics rail artifact producer + gate (J7Q-05).
//!
//! Boots one `E2eRuntime`, seeds a single document, and captures the read-only
//! operator-diagnosis surfaces (route/generation provenance, typed-error code,
//! perf metrics) machine-readably, writing the canonical artifacts under
//! `artifacts/search-quality/ops/latest/`. Authority behind
//! `just rust-verify-quality-ops`.
//!
//! Fail-closed: a surface that swallows its provenance (empty engine set,
//! generation mismatch, blank typed-error code, no metrics) records
//! `provenance_ok=false` and the rail exits non-zero — never a fabricated pass.

use std::path::PathBuf;
use std::process::ExitCode;

use quanta_index_searchd_harness::artifact::{GitHeadV1, HostV1};
use quanta_index_searchd_harness::ops::{run_ops_report, write_artifacts};

/// The exact head of a clean worktree, or a refusal: a verdict artifact
/// that cannot say which source it judged is not written (QI-BB-010).
fn provenance()
-> Result<(GitHeadV1, HostV1), quanta_index_searchd_harness::artifact::BenchProvenanceError> {
    Ok((
        GitHeadV1::resolve(std::path::Path::new("."))?,
        HostV1::observe()?,
    ))
}

fn parse_out_dir() -> PathBuf {
    let mut args = std::env::args().skip(1);
    match (args.next().as_deref(), args.next()) {
        (Some("--out-dir"), Some(path)) => PathBuf::from(path),
        _ => PathBuf::from("artifacts/search-quality/ops/latest"),
    }
}

#[expect(
    clippy::print_stderr,
    clippy::print_stdout,
    reason = "rail binary reports rail status on stdout/stderr by design"
)]
fn main() -> ExitCode {
    let out_dir = parse_out_dir();
    let (git_head, host) = match provenance() {
        Ok(value) => value,
        Err(err) => {
            eprintln!("ops_matrix: {err}");
            return ExitCode::FAILURE;
        }
    };

    let report = match run_ops_report() {
        Ok(report) => report,
        Err(err) => {
            eprintln!("ops_matrix: rail run failed: {err:#}");
            return ExitCode::FAILURE;
        }
    };

    if let Err(err) = write_artifacts(&report, &out_dir, git_head, host) {
        eprintln!(
            "ops_matrix: failed to write artifacts under {}: {err:#}",
            out_dir.display()
        );
        return ExitCode::FAILURE;
    }

    for snapshot in &report.snapshots {
        println!(
            "ops[{}] provenance_ok={} -- {}",
            snapshot.surface, snapshot.provenance_ok, snapshot.note,
        );
        if !snapshot.provenance_ok {
            eprintln!(
                "    surface `{}` swallowed provenance ({})",
                snapshot.surface, snapshot.operator_question
            );
        }
    }

    if report.passed {
        println!(
            "ops rail green (read-only diagnosis surfaces preserve route/generation/typed-error provenance)"
        );
        ExitCode::SUCCESS
    } else {
        eprintln!("ops rail RED: a diagnosis surface swallowed its provenance");
        ExitCode::FAILURE
    }
}
