//! `snippet_matrix` — snippet-quality rail artifact producer + gate (J7Q-02).
//!
//! Seeds the snippet fixture (phrase / regex / multi-hit / long-line), runs each
//! judged query through the product Native SDK, grades the emitted excerpt
//! against the hit-centered-window + bounded-length +
//! deterministic-truncation contract (NOT mere substring presence), writes the
//! canonical artifacts under `artifacts/search-quality/snippet/latest/`, and
//! exits non-zero on any per-query gate failure. The snippet is graded
//! as-emitted: the rail never post-processes the engine output to force a pass,
//! so an unwindowed/untruncated long-line snippet trips the gate honestly.
//! Authority behind `just rust-verify-quality-snippet`.

#[path = "common/artifact_out_dir.rs"]
mod artifact_out_dir;

use std::ffi::OsString;
use std::fs::File;
use std::os::unix::fs::PermissionsExt as _;
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context as _, Result as AnyResult};
use quanta_index_searchd_harness::artifact::{GitHeadV1, HostV1};
use quanta_index_searchd_harness::snippet::{
    invalidate_artifacts, run_native_snippet_report, write_artifacts,
};

struct CliArgs {
    out_dir: PathBuf,
    daemon_bin: Option<PathBuf>,
}

fn parse_args(args: impl IntoIterator<Item = OsString>) -> AnyResult<CliArgs> {
    let mut args = args.into_iter();
    let mut out_dir = None;
    let mut daemon_bin = None;
    while let Some(flag) = args.next() {
        let value = args
            .next()
            .filter(|value| !value.is_empty() && !value.as_encoded_bytes().starts_with(b"--"));
        match flag.to_str() {
            Some("--out-dir") if out_dir.is_none() => {
                out_dir = Some(value.context("--out-dir requires a directory path")?);
            }
            Some("--native-daemon-bin") if daemon_bin.is_none() => {
                daemon_bin =
                    Some(value.context("--native-daemon-bin requires an executable path")?);
            }
            _ => anyhow::bail!("unexpected or repeated snippet_matrix argument: {flag:?}"),
        }
    }
    let out_dir_args =
        out_dir.map_or_else(Vec::new, |path| vec![OsString::from("--out-dir"), path]);
    let out_dir = artifact_out_dir::parse(out_dir_args, "artifacts/search-quality/snippet/latest")?;
    let daemon_bin = daemon_bin.map(PathBuf::from);
    Ok(CliArgs {
        out_dir,
        daemon_bin,
    })
}

fn validate_daemon_bin(path: Option<&PathBuf>) -> AnyResult<&PathBuf> {
    let path = path.context("snippet_matrix requires --native-daemon-bin ABSOLUTE_EXECUTABLE")?;
    anyhow::ensure!(
        path.is_absolute()
            && path.is_file()
            && std::fs::metadata(path)?.permissions().mode() & 0o111 != 0,
        "--native-daemon-bin must be an absolute existing executable"
    );
    Ok(path)
}

/// The exact head of a clean worktree, or a refusal: a verdict artifact
/// that cannot say which source it judged is not written (QI-BB-010).
fn provenance()
-> Result<(GitHeadV1, HostV1), quanta_index_searchd_harness::artifact::BenchProvenanceError> {
    Ok((
        GitHeadV1::resolve(std::path::Path::new("."))?,
        HostV1::observe()?,
    ))
}

#[expect(
    clippy::print_stderr,
    clippy::print_stdout,
    reason = "rail binary reports rail status on stdout/stderr by design"
)]
fn main() -> ExitCode {
    let args = match parse_args(std::env::args_os().skip(1)) {
        Ok(args) => args,
        Err(err) => {
            eprintln!("snippet_matrix: {err}");
            return ExitCode::FAILURE;
        }
    };
    let output_lease = (|| -> AnyResult<File> {
        std::fs::create_dir_all(&args.out_dir)?;
        let directory = File::open(&args.out_dir)?;
        directory
            .try_lock()
            .context("cannot acquire snippet output directory lease")?;
        invalidate_artifacts(&args.out_dir)?;
        Ok(directory)
    })();
    let _output_lease = match output_lease {
        Ok(lease) => lease,
        Err(err) => {
            eprintln!("snippet_matrix: {err:#}");
            return ExitCode::FAILURE;
        }
    };
    let daemon_bin = match validate_daemon_bin(args.daemon_bin.as_ref()) {
        Ok(path) => path,
        Err(err) => {
            eprintln!("snippet_matrix: {err:#}");
            return ExitCode::FAILURE;
        }
    };
    let (git_head, host) = match provenance() {
        Ok(value) => value,
        Err(err) => {
            eprintln!("snippet_matrix: {err}");
            return ExitCode::FAILURE;
        }
    };

    let report = match run_native_snippet_report(daemon_bin) {
        Ok(report) => report,
        Err(err) => {
            eprintln!("snippet_matrix: rail run failed: {err:#}");
            return ExitCode::FAILURE;
        }
    };

    if let Err(err) = write_artifacts(&report, &args.out_dir, git_head, host) {
        eprintln!(
            "snippet_matrix: failed to write artifacts under {}: {err:#}",
            args.out_dir.display()
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
