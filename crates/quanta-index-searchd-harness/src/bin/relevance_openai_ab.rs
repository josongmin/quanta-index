//! `relevance_openai_ab` — local OpenAI-vs-Hash semantic capture lane.
//!
//! Runs the semantic judged fixture twice: once on the deterministic `Hash`
//! embedder and once on the env-resolved `OpenAI` profile, then writes the
//! advisory artifacts under `artifacts/search-quality/relevance/openai-ab/`.
//! This is intentionally NOT a blocking CI rail: it is a local discriminative
//! probe for paraphrase-quality deltas and provider request-shaping telemetry.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::Result as AnyResult;
use quanta_index_searchd_harness::artifact::{GitHeadV1, HostV1};
use quanta_index_searchd_harness::relevance::corpus::SemanticIntentKind;
use quanta_index_searchd_harness::relevance::report::{
    OpenAiSemanticAbReport, openai_profile_from_env_for_relevance_ab,
    run_openai_semantic_ab_report, write_openai_ab_artifacts,
};

fn parse_out_dir() -> PathBuf {
    let mut args = std::env::args().skip(1);
    match (args.next().as_deref(), args.next()) {
        (Some("--out-dir"), Some(path)) => PathBuf::from(path),
        _ => PathBuf::from("artifacts/search-quality/relevance/openai-ab/latest"),
    }
}

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
    let report = run_openai_semantic_ab_report(profile.clone())?;
    write_openai_ab_artifacts(&report, &profile, out_dir, git_head, host)?;
    Ok(report)
}

#[expect(
    clippy::print_stderr,
    clippy::print_stdout,
    reason = "local capture binary reports capture status on stdout/stderr by design"
)]
fn main() -> ExitCode {
    let out_dir = parse_out_dir();
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
