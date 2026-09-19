//! `ann_matrix` — the ANN recall/latency rail artifact producer + gate
//! (QI-BB-027 완료 기준 #3).
//!
//! Seals the default tier (4,096 rows × 64 dimensions) through the semantic
//! adapter in a private temporary state root, asks 64 neighbour queries,
//! scores them against an exhaustive cosine oracle and writes
//! `artifacts/search-quality/ann/latest/summary.json`: the
//! `BenchArtifactV1` of the exact head, with recall@k, the latency
//! percentiles, the build time, the index bytes, the dense lane the seal
//! proved and the normalization policy. Authority behind
//! `just rust-verify-quality-ann`.
//!
//! Fail-closed: provenance is resolved before anything runs, and recall
//! below the floor, a score that is not the exact cosine, or a short page
//! is a non-zero exit after the artifact is written.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::Result as AnyResult;
use quanta_index_searchd_harness::ann::{
    AnnRailConfig, AnnReport, run_ann_report, write_artifacts,
};
use quanta_index_searchd_harness::artifact::{GitHeadV1, HostV1};

fn parse_out_dir() -> PathBuf {
    let mut args = std::env::args().skip(1);
    match (args.next().as_deref(), args.next()) {
        (Some("--out-dir"), Some(path)) => PathBuf::from(path),
        _ => PathBuf::from("artifacts/search-quality/ann/latest"),
    }
}

fn run(out_dir: &Path) -> AnyResult<AnnReport> {
    // Provenance first: a run that cannot be attributed is not started.
    let git_head = GitHeadV1::resolve(Path::new("."))?;
    let host = HostV1::observe()?;
    let state_root = tempfile::tempdir()?;
    let report = run_ann_report(AnnRailConfig::DEFAULT, state_root.path())?;
    write_artifacts(&report, out_dir, git_head, host)?;
    Ok(report)
}

#[expect(
    clippy::print_stderr,
    clippy::print_stdout,
    reason = "rail binary reports rail status on stdout/stderr by design"
)]
fn main() -> ExitCode {
    let out_dir = parse_out_dir();
    let report = match run(&out_dir) {
        Ok(report) => report,
        Err(err) => {
            eprintln!("ann_matrix: {err:#}");
            return ExitCode::FAILURE;
        }
    };
    let config = report.config;
    println!(
        "ann rows={} width={} queries={} k={} recall_at_k={:.4} build_ms={:.1} index_bytes={} exact_scores={} short_pages={}",
        config.rows,
        config.width,
        config.queries,
        config.k,
        report.recall_at_k,
        report.build_ms,
        report.index_bytes,
        report.exact_scores,
        report.short_pages,
    );
    println!("ann {}", report.dense_lane);
    if report.passed {
        println!("ann rail green (recall floor {:.2})", config.recall_floor);
        ExitCode::SUCCESS
    } else {
        eprintln!("ann rail RED: recall, exact scores or page length below the gate");
        ExitCode::FAILURE
    }
}
