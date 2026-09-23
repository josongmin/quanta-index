//! Explicit workspace mutation to generation-pinned lexical visibility rail.

#[path = "../freshness.rs"]
mod freshness;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Result as AnyResult, anyhow};
use quanta_index_searchd_harness::artifact::{GitHeadV1, HostV1};

struct Cli {
    out_dir: PathBuf,
    samples: u32,
}

fn parse_args() -> AnyResult<Cli> {
    let mut out_dir = PathBuf::from("artifacts/search-quality/freshness/latest");
    let mut samples = 20;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--out-dir" => {
                out_dir = PathBuf::from(
                    args.next()
                        .ok_or_else(|| anyhow!("--out-dir needs a path"))?,
                );
            }
            "--samples" => {
                let raw = args
                    .next()
                    .ok_or_else(|| anyhow!("--samples needs a value"))?;
                samples = raw
                    .parse()
                    .map_err(|err| anyhow!("invalid --samples {raw:?}: {err}"))?;
            }
            other => return Err(anyhow!("unknown argument {other:?}")),
        }
    }
    Ok(Cli { out_dir, samples })
}

fn run(cli: &Cli) -> AnyResult<freshness::FreshnessReport> {
    let head = GitHeadV1::resolve(Path::new("."))?;
    let host = HostV1::observe()?;
    let report = freshness::run(cli.samples)?;
    freshness::write_artifact(&report, &cli.out_dir, head, host)?;
    Ok(report)
}

#[expect(
    clippy::print_stderr,
    clippy::print_stdout,
    reason = "rail binary reports status on stdout/stderr"
)]
fn main() -> ExitCode {
    let outcome = parse_args().and_then(|cli| run(&cli));
    match outcome {
        Ok(report) => {
            println!(
                "freshness rail green: {} independent workspace samples; update/delete/rename correctness and historical generation coherence verified",
                report.samples.len()
            );
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("freshness_matrix: {err:#}");
            ExitCode::FAILURE
        }
    }
}
