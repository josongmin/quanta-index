//! Exact corpus diagnostic through the product Native UDS SDK.

use std::path::PathBuf;

use anyhow::{Context as _, Result};
use quanta_index_searchd::DiagnosticCaptureLimitsV1;
use quanta_index_searchd_harness::scale::{NativeCorpusInput, measure_native_corpus};

fn run() -> Result<()> {
    let mut source_root = None;
    let mut manifest = None;
    let mut manifest_sha256 = None;
    let mut repository_commit = None;
    let mut query_path = None;
    let mut query_token = None;
    let mut daemon_bin = None;
    let mut output_root = None;
    let mut trace_mode = None;
    let mut trace_max_output_bytes = None;
    let mut args = std::env::args().skip(1);
    while let Some(flag) = args.next() {
        let value = args
            .next()
            .with_context(|| format!("{flag} requires a value"))?;
        let slot = match flag.as_str() {
            "--source-root" => &mut source_root,
            "--manifest" => &mut manifest,
            "--manifest-sha256" => &mut manifest_sha256,
            "--repository-commit" => &mut repository_commit,
            "--query-path" => &mut query_path,
            "--query-token" => &mut query_token,
            "--native-daemon-bin" => &mut daemon_bin,
            "--output-root" => &mut output_root,
            "--trace-mode" => &mut trace_mode,
            "--trace-max-output-bytes" => &mut trace_max_output_bytes,
            _ => anyhow::bail!("unknown argument {flag}"),
        };
        anyhow::ensure!(slot.replace(value).is_none(), "duplicate argument {flag}");
    }
    let source_root = PathBuf::from(source_root.context("--source-root required")?);
    let manifest = PathBuf::from(manifest.context("--manifest required")?);
    let manifest_sha256: String = manifest_sha256.context("--manifest-sha256 required")?;
    let repository_commit: String = repository_commit.context("--repository-commit required")?;
    let query_path: String = query_path.context("--query-path required")?;
    let query_token: String = query_token.context("--query-token required")?;
    let daemon_bin = PathBuf::from(daemon_bin.context("--native-daemon-bin required")?);
    let output_root = PathBuf::from(output_root.context("--output-root required")?);
    let trace_max_output_bytes = match trace_mode.as_deref().unwrap_or("off") {
        "off" => {
            anyhow::ensure!(
                trace_max_output_bytes.is_none(),
                "--trace-max-output-bytes requires --trace-mode record"
            );
            None
        }
        "record" => {
            let bytes = trace_max_output_bytes
                .as_deref()
                .map(str::parse::<u64>)
                .transpose()
                .context("--trace-max-output-bytes must be a positive u64")?
                .unwrap_or_else(|| DiagnosticCaptureLimitsV1::default().max_output_bytes);
            let _limits = DiagnosticCaptureLimitsV1::default()
                .with_max_output_bytes(bytes)
                .context("--trace-max-output-bytes is outside capture limits")?;
            Some(bytes)
        }
        _ => anyhow::bail!("--trace-mode must be off or record"),
    };
    let result = measure_native_corpus(&NativeCorpusInput {
        source_root: &source_root,
        manifest: &manifest,
        manifest_sha256: &manifest_sha256,
        repository_commit: &repository_commit,
        query_path: &query_path,
        query_token: &query_token,
        daemon_bin: &daemon_bin,
        output_root: &output_root,
        trace_max_output_bytes,
    })
    .with_context(|| format!("Native corpus run output root {}", output_root.display()))?;
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}

fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("native_corpus FAILED: {error:#}");
            std::process::ExitCode::FAILURE
        }
    }
}
