//! Paired SDK quality replay of a blind pack on an already sealed generation.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use quanta_index_contract::{
    ExecutionOutcomeV2, GenerationPin, GoTestTraceFileLaneWitnessV1, LexicalCandidate,
    ManifestGeneration, QueryResultWindowV2, RepoId, RevisionId, TextRankUnit,
};
use quanta_index_retrieval_bench::query_plan::{NlPlanConfig, QueryInputPolicy, QueryPlan};
use quanta_index_retrieval_bench::record::load_query_pack;
use quanta_index_retrieval_bench::{BenchError, BenchResult, sha256_hex};
use quanta_index_sdk::{ConnectOptions, QuantaIndex};
use serde_json::{Value, json};

use super::{Args, optional_u64, reject_unknown, required, write_json_bound};

fn bound_window(
    route: &str,
    expected: &GenerationPin,
    actual: &GenerationPin,
    window: &QueryResultWindowV2,
) -> BenchResult<()> {
    if actual != expected {
        return Err(BenchError::Protocol(format!("{route}: generation drift")));
    }
    if matches!(
        window.outcome(),
        ExecutionOutcomeV2::InterruptedPartial { .. } | ExecutionOutcomeV2::CappedUnknown { .. }
    ) {
        return Err(BenchError::Protocol(format!(
            "{route}: execution did not produce a complete bounded window"
        )));
    }
    Ok(())
}

fn row(candidate: &LexicalCandidate) -> Value {
    json!({
        "candidate_id": candidate.candidate_id,
        "source_repo_id": candidate.source_repo_id,
        "path": candidate.repo_relative_path,
        "score": candidate.score,
    })
}

fn witness(witness: &GoTestTraceFileLaneWitnessV1) -> Value {
    json!({
        "candidate_id": witness.candidate.candidate_id,
        "source_repo_id": witness.candidate.source_repo_id,
        "path": witness.candidate.repo_relative_path,
        "chunk_rank": witness.chunk_rank,
        "file_rank": witness.file_rank,
        "raw_score": witness.candidate.score,
    })
}

fn trace_files(
    client: &QuantaIndex,
    pin: &GenerationPin,
    plan: &QueryPlan,
    lane_top_k: u32,
    failure_excerpt: &str,
) -> BenchResult<Value> {
    let start = Instant::now();
    let response = client
        .search()
        .hybrid()
        .native(&plan.lexical_request)
        .semantic_text(&plan.semantic_text)
        .pinned(pin.clone())
        .top_k(lane_top_k)
        .go_test_trace_file(failure_excerpt, 20)
        .map_err(|error| BenchError::Sdk(error.to_string()))?;
    let query_micros = start.elapsed().as_micros();
    bound_window(
        "trace lexical",
        pin,
        &response.generation,
        &response.lexical_window,
    )?;
    bound_window(
        "trace dense",
        pin,
        &response.generation,
        &response.dense_window,
    )?;
    let candidates = response
        .results
        .iter()
        .map(|hit| {
            let candidate = hit.representative().ok_or_else(|| {
                BenchError::Protocol("trace file response lacks a lane witness".to_string())
            })?;
            Ok(json!({
                "file": hit.file,
                "source_repo_id": hit.file.source_repo_id,
                "path": hit.file.repo_relative_path,
                "candidate_id": candidate.candidate_id,
                "file_rrf_score": hit.file_rrf_score,
                "production_source": hit.production_source,
                "source_frame": hit.source_frame,
                "lexical": hit.lexical.as_ref().map(witness),
                "dense": hit.dense.as_ref().map(witness),
            }))
        })
        .collect::<BenchResult<Vec<_>>>()?;
    Ok(json!({
        "profile": "go_trace_file_rrf_v1",
        "sdk_query_micros": query_micros,
        "frame_class": response.frame_class.as_str(),
        "failure_excerpt_sha256": sha256_hex(failure_excerpt.as_bytes()),
        "lexical_window": response.lexical_window,
        "dense_window": response.dense_window,
        "candidates": candidates,
    }))
}

fn paired_query(
    client: &QuantaIndex,
    pin: &GenerationPin,
    plan: &QueryPlan,
    top_k: u32,
    failure_excerpt: Option<&str>,
) -> BenchResult<Value> {
    let sdk_error = |error: quanta_index_sdk::SdkError| BenchError::Sdk(error.to_string());
    let start = Instant::now();
    let lexical = client
        .lexical()
        .query()
        .native(&plan.lexical_request)
        .pinned(pin.clone())
        .top_k(top_k)
        .execute()
        .map_err(sdk_error)?;
    let lexical_micros = start.elapsed().as_micros();
    bound_window("lexical", pin, &lexical.generation, &lexical.window)?;
    if lexical.rank_unit != TextRankUnit::Chunk {
        return Err(BenchError::Protocol(
            "trace-profile expects chunk-ranked lexical candidates".to_string(),
        ));
    }
    let start = Instant::now();
    let semantic = client
        .semantic()
        .query()
        .text(&plan.semantic_text)
        .pinned(pin.clone())
        .top_k(top_k)
        .execute()
        .map_err(sdk_error)?;
    let semantic_micros = start.elapsed().as_micros();
    bound_window("semantic", pin, &semantic.generation, &semantic.window)?;
    let start = Instant::now();
    let hybrid = client
        .search()
        .hybrid()
        .native(&plan.lexical_request)
        .semantic_text(&plan.semantic_text)
        .pinned(pin.clone())
        .top_k(top_k)
        .execute()
        .map_err(sdk_error)?;
    let hybrid_micros = start.elapsed().as_micros();
    bound_window("hybrid", pin, &hybrid.generation, &hybrid.window)?;
    let trace_files = failure_excerpt
        .map(|text| trace_files(client, pin, plan, top_k, text))
        .transpose()?;
    Ok(json!({
        "query_identity": {
            "original_query_sha256": plan.original_query_sha256,
            "effective_lexical_request_sha256": plan.effective_lexical_request_sha256,
            "semantic_text_sha256": plan.semantic_text_sha256,
        },
        "lexical": {
            "query_micros": lexical_micros,
            "window": lexical.window,
            "candidates": lexical.results.iter().map(row).collect::<Vec<_>>(),
        },
        "semantic": {
            "query_micros": semantic_micros,
            "window": semantic.window,
            "candidates": semantic.results.iter().map(row).collect::<Vec<_>>(),
        },
        "hybrid": {
            "query_micros": hybrid_micros,
            "window": hybrid.window,
            "candidates": hybrid.results.iter().map(|hit| json!({
                "candidate_id": hit.candidate.candidate_id,
                "source_repo_id": hit.candidate.source_repo_id,
                "path": hit.candidate.repo_relative_path,
                "score": hit.fused_score,
                "contributions": hit.contributions,
            })).collect::<Vec<_>>(),
        },
        "trace_files": trace_files,
    }))
}

pub(super) fn run(args: &Args) -> BenchResult<()> {
    reject_unknown(
        args,
        &[
            "query-pack",
            "state-root",
            "repo-id",
            "generation",
            "nl-max-tokens",
            "io-timeout-secs",
            "out",
        ],
    )?;
    let pack = load_query_pack(&PathBuf::from(required(args, "query-pack")?))?;
    if pack.contract_top_k != 100 {
        return Err(BenchError::Config(
            "trace-profile requires a pinned 100-candidate lane budget".to_string(),
        ));
    }
    let out = PathBuf::from(required(args, "out")?);
    let state = PathBuf::from(required(args, "state-root")?);
    let repo_id = RepoId::new(required(args, "repo-id")?)
        .map_err(|error| BenchError::Config(error.to_string()))?;
    let revision_id = RevisionId::new(pack.repository_commit.as_str())
        .map_err(|error| BenchError::Config(error.to_string()))?;
    let generation = required(args, "generation")?
        .parse::<u64>()
        .ok()
        .filter(|generation| *generation > 0)
        .ok_or_else(|| BenchError::Config("expected a positive generation".to_string()))?;
    let pin = GenerationPin::new(repo_id, revision_id, ManifestGeneration::new(generation));
    let config = NlPlanConfig {
        max_tokens: super::optional_usize(
            args,
            "nl-max-tokens",
            NlPlanConfig::default().max_tokens,
        )?,
        ..NlPlanConfig::default()
    };
    let timeout = Duration::from_secs(optional_u64(args, "io-timeout-secs", 60)?);
    let client = QuantaIndex::connect(
        ConnectOptions::from_state_root(&state).with_request_io_timeout(timeout),
    )
    .map_err(|error| BenchError::Sdk(error.to_string()))?;
    let tasks = pack
        .tasks
        .iter()
        .map(|task| {
            let plan = task
                .plan(QueryInputPolicy::NaturalLanguage, &config)
                .map_err(|error| BenchError::Config(error.to_string()))?;
            let mut result = paired_query(
                &client,
                &pin,
                &plan,
                pack.contract_top_k,
                task.semantic_input
                    .as_ref()
                    .map(|input| input.failure_excerpt()),
            )?;
            result["task_id"] = json!(task.task_id);
            Ok(result)
        })
        .collect::<BenchResult<Vec<_>>>()?;
    let _digest = write_json_bound(
        &out,
        &json!({
            "schema_version": 1,
            "scope": "sealed-generation-sdk-quality-diagnostic",
            "pack_sha256": pack.pack_sha256,
            "lane_top_k": pack.contract_top_k,
            "generation": pin,
            "tasks": tasks,
        }),
    )?;
    Ok(())
}
