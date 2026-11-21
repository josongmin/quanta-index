//! `relevance_openai_ab` — local OpenAI-vs-Hash semantic capture lane.
//!
//! Publishes and queries the semantic fixture through the SDK against two fresh
//! processes of `QUANTA_INDEX_LEGACY_SEARCHD_BIN` pinned by
//! `QUANTA_INDEX_LEGACY_SEARCHD_SHA256`. The `Hash` arm is deterministic and the
//! `OpenAI` arm uses the env-resolved profile and the daemon's provider telemetry.
//! Writes raw sidecars into a new staging directory. The capture adapter
//! validates and publishes the three files together.
//! This is intentionally NOT a blocking CI rail: it is a local discriminative
//! probe for paraphrase-quality deltas and provider request-shaping telemetry.

#[path = "common/artifact_out_dir.rs"]
mod artifact_out_dir;

use std::path::Path;
use std::process::ExitCode;

use anyhow::{Context as _, Result as AnyResult};
use quanta_index_searchd::app::SemanticEmbedderProfile;
use quanta_index_searchd_harness::artifact::{GitHeadV1, HostV1};
use quanta_index_searchd_harness::relevance::corpus::SemanticIntentKind;
use quanta_index_searchd_harness::relevance::report::{
    OpenAiSemanticAbReport, describe_openai_ab_inputs, openai_profile_from_env_for_relevance_ab,
    run_openai_semantic_ab_report, write_openai_ab_artifacts,
};

fn intent_kind_label(kind: SemanticIntentKind) -> &'static str {
    match kind {
        SemanticIntentKind::ExactToken => "exact_token",
        SemanticIntentKind::Paraphrase => "paraphrase",
    }
}

fn run(out_dir: &Path) -> AnyResult<OpenAiSemanticAbReport> {
    // Provenance first: a run that cannot be attributed is not started.
    let git_head = GitHeadV1::resolve(Path::new("."))?;
    let host = HostV1::observe()?;
    let profile = openai_profile_from_env_for_relevance_ab()
        .map_err(|err| anyhow::anyhow!("invalid OpenAI profile env: {err:#}"))?;
    create_staging_directory(out_dir)?;
    let report = run_openai_semantic_ab_report(profile)?;
    write_openai_ab_artifacts(&report, out_dir, git_head, host)?;
    Ok(report)
}

fn create_staging_directory(out_dir: &Path) -> AnyResult<()> {
    if let Some(parent) = out_dir.parent().filter(|path| !path.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::create_dir(out_dir).with_context(|| {
        format!(
            "OpenAI A/B requires a fresh staging directory: {}",
            out_dir.display()
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openai_ab_refuses_existing_staging_directory() -> AnyResult<()> {
        let root = tempfile::tempdir()?;
        let stage = root.path().join("parent").join("capture");
        create_staging_directory(&stage)?;
        anyhow::ensure!(
            create_staging_directory(&stage).is_err(),
            "an existing empty stage accepted a second producer"
        );
        let summary = stage.join("summary.json");
        std::fs::write(&summary, b"previous capture")?;
        anyhow::ensure!(
            create_staging_directory(&stage).is_err(),
            "an existing populated stage accepted a second producer"
        );
        anyhow::ensure!(
            std::fs::read(summary)? == b"previous capture",
            "the original capture changed"
        );
        Ok(())
    }
}

#[expect(
    clippy::print_stderr,
    clippy::print_stdout,
    reason = "local capture binary reports capture status on stdout/stderr by design"
)]
fn main() -> ExitCode {
    let args = std::env::args_os().skip(1).collect::<Vec<_>>();
    if matches!(args.as_slice(), [arg] if arg == "--print-inputs") {
        return match SemanticEmbedderProfile::openai_for_input_review_from_env()
            .and_then(|profile| describe_openai_ab_inputs(&profile))
        {
            Ok(inputs) => {
                println!("{inputs}");
                ExitCode::SUCCESS
            }
            Err(err) => {
                eprintln!("relevance_openai_ab: {err:#}");
                ExitCode::FAILURE
            }
        };
    }

    let out_dir = match artifact_out_dir::parse(
        args,
        "artifacts/search-quality/relevance/openai-ab/latest",
    ) {
        Ok(path) => path,
        Err(err) => {
            eprintln!("relevance_openai_ab: {err}");
            return ExitCode::FAILURE;
        }
    };
    let report = match run(&out_dir) {
        Ok(report) => report,
        Err(err) => {
            eprintln!("relevance_openai_ab: {err:#}");
            return ExitCode::FAILURE;
        }
    };

    for case in &report.cases {
        println!(
            "relevance-openai-ab[{}]: kind={} hash_top1={} openai_top1={} delta_mrr={:.4} delta_ndcg={:.4} delta_recall={:.4}",
            case.id,
            intent_kind_label(case.intent_kind),
            case.hash.top1_is_on_topic,
            case.openai.top1_is_on_topic,
            case.openai.mrr_at_10 - case.hash.mrr_at_10,
            case.openai.ndcg_at_10 - case.hash.ndcg_at_10,
            case.openai.recall_at_20 - case.hash.recall_at_20,
        );
    }
    println!(
        "relevance-openai-ab provider: texts={} cache_hits={} miss_texts={} http_requests={} retries={} retryable_statuses={} transport_errors={}",
        report.provider_stats.total_texts_observed,
        report.provider_stats.cache_hits,
        report.provider_stats.distinct_miss_texts,
        report.provider_stats.http_request_count,
        report.provider_stats.retry_count,
        report.provider_stats.retryable_status_count,
        report.provider_stats.transport_error_count,
    );
    println!(
        "relevance-openai-ab artifacts written to {}",
        out_dir.display()
    );
    ExitCode::SUCCESS
}
