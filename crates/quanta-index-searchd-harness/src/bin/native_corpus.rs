//! Exact corpus diagnostic through the product Native UDS SDK.

use std::{io::Write as _, path::PathBuf};

use anyhow::{Context as _, Result};
use quanta_index_searchd::DiagnosticCaptureLimitsV1;
use quanta_index_searchd_harness::native_index::{
    NativeCorpusInput, NativeCorpusTextLayout, NativeSemanticCorpusInput, measure_native_corpus,
    measure_native_corpus_with_semantic,
};

fn hex16(value: &str, name: &str) -> Result<[u8; 16]> {
    anyhow::ensure!(
        value.len() == 32
            && value
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()),
        "{name} must be 32 lowercase hex digits"
    );
    let mut out = [0; 16];
    for (slot, pair) in out.iter_mut().zip(value.as_bytes().chunks_exact(2)) {
        *slot = u8::from_str_radix(std::str::from_utf8(pair)?, 16)?;
    }
    Ok(out)
}

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
    let mut text_layout = None;
    let mut semantic_native_config = None;
    let mut semantic_native_config_sha256 = None;
    let mut semantic_native_socket = None;
    let mut semantic_instance = None;
    let mut semantic_kind = None;
    let mut semantic_group = None;
    let mut semantic_schema = None;
    let mut semantic_format = None;
    let mut semantic_max_source_bytes = None;
    let mut source_universe_sha256 = None;
    let mut source_binding_sha256 = None;
    let mut semantic_wait_seconds = None;
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
            "--text-layout" => &mut text_layout,
            "--semantic-native-config" => &mut semantic_native_config,
            "--semantic-native-config-sha256" => &mut semantic_native_config_sha256,
            "--semantic-native-socket" => &mut semantic_native_socket,
            "--semantic-instance" => &mut semantic_instance,
            "--semantic-kind" => &mut semantic_kind,
            "--semantic-group" => &mut semantic_group,
            "--semantic-schema" => &mut semantic_schema,
            "--semantic-format" => &mut semantic_format,
            "--semantic-max-source-bytes" => &mut semantic_max_source_bytes,
            "--source-universe-sha256" => &mut source_universe_sha256,
            "--source-binding-sha256" => &mut source_binding_sha256,
            "--semantic-wait-seconds" => &mut semantic_wait_seconds,
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
    let text_layout = match text_layout.as_deref().unwrap_or("tree-v1") {
        "tree-v1" => NativeCorpusTextLayout::TreeV1,
        "segmented-full-v1" => NativeCorpusTextLayout::SegmentedFullV1,
        "segmented-layered-v2" => NativeCorpusTextLayout::SegmentedLayeredV2,
        _ => anyhow::bail!(
            "--text-layout must be tree-v1, segmented-full-v1, or segmented-layered-v2"
        ),
    };
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
    let semantic_requested = [
        semantic_native_config.as_ref(),
        semantic_native_config_sha256.as_ref(),
        semantic_native_socket.as_ref(),
        semantic_instance.as_ref(),
        semantic_kind.as_ref(),
        semantic_group.as_ref(),
        semantic_schema.as_ref(),
        semantic_format.as_ref(),
        semantic_max_source_bytes.as_ref(),
        source_universe_sha256.as_ref(),
        source_binding_sha256.as_ref(),
        semantic_wait_seconds.as_ref(),
    ]
    .iter()
    .any(Option::is_some);
    let semantic_config_path = semantic_native_config.map(PathBuf::from);
    let semantic_socket_path = semantic_native_socket.map(PathBuf::from);
    let semantic = if semantic_requested {
        Some(NativeSemanticCorpusInput {
            config_path: semantic_config_path
                .as_deref()
                .context("--semantic-native-config required")?,
            config_sha256: semantic_native_config_sha256
                .as_deref()
                .context("--semantic-native-config-sha256 required")?,
            socket_path: semantic_socket_path
                .as_deref()
                .context("--semantic-native-socket required")?,
            instance: hex16(
                semantic_instance
                    .as_deref()
                    .context("--semantic-instance required")?,
                "semantic instance",
            )?,
            kind: hex16(
                semantic_kind
                    .as_deref()
                    .context("--semantic-kind required")?,
                "semantic kind",
            )?,
            group: hex16(
                semantic_group
                    .as_deref()
                    .context("--semantic-group required")?,
                "semantic group",
            )?,
            schema: hex16(
                semantic_schema
                    .as_deref()
                    .context("--semantic-schema required")?,
                "semantic schema",
            )?,
            format: hex16(
                semantic_format
                    .as_deref()
                    .context("--semantic-format required")?,
                "semantic format",
            )?,
            max_source_bytes: semantic_max_source_bytes
                .as_deref()
                .context("--semantic-max-source-bytes required")?
                .parse()?,
            source_universe_sha256: source_universe_sha256
                .as_deref()
                .context("--source-universe-sha256 required")?,
            source_binding_sha256: source_binding_sha256
                .as_deref()
                .context("--source-binding-sha256 required")?,
            wait_seconds: semantic_wait_seconds
                .as_deref()
                .context("--semantic-wait-seconds required")?
                .parse()?,
        })
    } else {
        None
    };
    let input = NativeCorpusInput {
        source_root: &source_root,
        manifest: &manifest,
        manifest_sha256: &manifest_sha256,
        repository_commit: &repository_commit,
        query_path: &query_path,
        query_token: &query_token,
        daemon_bin: &daemon_bin,
        output_root: &output_root,
        trace_max_output_bytes,
        text_layout,
    };
    let result = match semantic.as_ref() {
        Some(semantic) => measure_native_corpus_with_semantic(&input, semantic),
        None => measure_native_corpus(&input),
    }
    .with_context(|| format!("Native corpus run output root {}", output_root.display()))?;
    let mut output = std::io::stdout().lock();
    serde_json::to_writer_pretty(&mut output, &result)?;
    writeln!(output)?;
    Ok(())
}

fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            let _diagnostic = writeln!(std::io::stderr().lock(), "native_corpus FAILED: {error:#}");
            std::process::ExitCode::FAILURE
        }
    }
}
