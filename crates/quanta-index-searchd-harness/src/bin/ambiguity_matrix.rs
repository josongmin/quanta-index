//! `ambiguity_matrix` — repairability rail artifact producer + gate (J7Q-06).
//!
//! Snapshots the typed repair payloads the search-plane wire boundary emits for
//! every bridge error code, enforces the repairability invariants (repairable
//! codes carry non-empty alternatives + a docs anchor; the internal code carries
//! none; classes stay distinct families; payloads round-trip the wire codec),
//! writes the canonical artifacts under
//! `artifacts/search-quality/ambiguity/latest/`, and exits non-zero on any
//! invariant failure. Authority behind `just rust-verify-quality-ambiguity`.

use std::process::ExitCode;

use quanta_index_searchd_harness::ambiguity::{run_ambiguity_report, write_artifacts};
use quanta_index_searchd_harness::artifact::{GitHeadV1, HostV1};

/// The exact head of a clean worktree, or a refusal: a verdict artifact
/// that cannot say which source it judged is not written (QI-BB-010).
fn provenance()
-> Result<(GitHeadV1, HostV1), quanta_index_searchd_harness::artifact::BenchProvenanceError> {
    Ok((
        GitHeadV1::resolve(std::path::Path::new("."))?,
        HostV1::observe()?,
    ))
}

#[path = "common/artifact_out_dir.rs"]
mod artifact_out_dir;

const DEFAULT_OUT_DIR: &str = "artifacts/search-quality/ambiguity/latest";

#[expect(
    clippy::print_stderr,
    clippy::print_stdout,
    reason = "rail binary reports rail status on stdout/stderr by design"
)]
fn main() -> ExitCode {
    let out_dir = match artifact_out_dir::parse(std::env::args_os().skip(1), DEFAULT_OUT_DIR) {
        Ok(path) => path,
        Err(err) => {
            eprintln!("ambiguity_matrix: {err}");
            return ExitCode::FAILURE;
        }
    };
    let (git_head, host) = match provenance() {
        Ok(value) => value,
        Err(err) => {
            eprintln!("ambiguity_matrix: {err}");
            return ExitCode::FAILURE;
        }
    };
    let report = run_ambiguity_report();

    if let Err(err) = write_artifacts(&report, &out_dir, git_head, host) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::{ffi::OsString, path::PathBuf};

    #[test]
    fn invalid_arguments_cannot_select_the_default_artifact_directory() {
        assert_eq!(
            artifact_out_dir::parse([], DEFAULT_OUT_DIR).ok(),
            Some(PathBuf::from("artifacts/search-quality/ambiguity/latest"))
        );
        assert_eq!(
            artifact_out_dir::parse(
                ["--out-dir", "custom output"].map(OsString::from),
                DEFAULT_OUT_DIR,
            )
            .ok(),
            Some(PathBuf::from("custom output"))
        );
        let invalid: [&[&str]; 5] = [
            &["--out-dir"],
            &["--out-dir", ""],
            &["--out-dir", "--help"],
            &["--output-dir", "wrong"],
            &["--out-dir", "custom", "extra"],
        ];
        for args in invalid {
            assert!(
                artifact_out_dir::parse(args.iter().copied().map(OsString::from), DEFAULT_OUT_DIR,)
                    .is_err(),
                "{args:?}"
            );
        }
    }
}
