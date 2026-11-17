//! Explicit generic file retrieval diagnostic on a sealed generation.
//! This is a separate wire/result unit from the historical chunk profile.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use quanta_index_contract::{
    GenerationPin, LexicalCandidate, ManifestGeneration, QueryConstraintSetV1, QueryResultWindowV2,
    RepoId, RevisionId, SEMANTIC_WORK_OPERATIONAL_CAP_V1, SemanticQueryRequest,
    SemanticWorkBoundedQueryRequestV1, TextQueryResponse, TextRankUnit,
    hybrid_file_candidate_pool_sha256_v1, validate_public_top_k,
};
use quanta_index_retrieval_bench::query_plan::{
    NlPlanConfig, QueryInputPolicy, QueryPlan, execution_profile_id, plan_query,
};
use quanta_index_retrieval_bench::record::{
    GENERIC_SEMANTIC_REPRESENTATION_POLICY, load_query_pack_with_file_digest,
};
use quanta_index_retrieval_bench::sdk::top_k_window_scoreable;
use quanta_index_retrieval_bench::{BenchError, BenchResult};
use quanta_index_sdk::{ConnectOptions, QuantaIndex};
use serde_json::{Value, json};

use super::{Args, optional_u64, reject_unknown, required, write_json_bound};

fn check_window(
    name: &str,
    pin: &GenerationPin,
    actual: &GenerationPin,
    window: &QueryResultWindowV2,
    cap: u32,
    rows: usize,
) -> BenchResult<()> {
    if actual != pin || window.returned() > cap {
        return Err(BenchError::Protocol(format!(
            "{name}: generation or returned lane cap drift"
        )));
    }
    if !top_k_window_scoreable(window.outcome(), window.returned(), cap) {
        return Err(BenchError::Protocol(format!(
            "{name}: incomplete bounded lane window"
        )));
    }
    if usize::try_from(window.returned()) != Ok(rows) {
        return Err(BenchError::Protocol(format!(
            "{name}: returned window disagrees with captured rows"
        )));
    }
    Ok(())
}

fn chunk(row: &LexicalCandidate) -> Value {
    json!({
        "candidate_id": row.candidate_id,
        "source_repo_id": row.source_repo_id,
        "path": row.repo_relative_path,
        "score": row.score,
    })
}

struct LexicalCapture {
    response: TextQueryResponse,
    query_micros: u128,
}

impl LexicalCapture {
    fn evidence(&self, plan: &QueryPlan, cap: u32) -> Value {
        json!({
            "query_micros": self.query_micros,
            "execution_profile_id": execution_profile_id(plan.policy),
            "effective_lexical_request_sha256": plan.effective_lexical_request_sha256,
            "rank_unit": self.response.rank_unit,
            "top_k": cap,
            "window": self.response.window,
            "candidates": self.response.results.iter().map(chunk).collect::<Vec<_>>(),
        })
    }
}

fn capture_lexical(
    client: &QuantaIndex,
    pin: &GenerationPin,
    plan: &QueryPlan,
    cap: u32,
    rank_unit: TextRankUnit,
) -> BenchResult<LexicalCapture> {
    let start = Instant::now();
    let response = client
        .lexical()
        .query()
        .native(&plan.lexical_request)
        .pinned(pin.clone())
        .top_k(cap)
        .execute()
        .map_err(|error| BenchError::Sdk(error.to_string()))?;
    let query_micros = start.elapsed().as_micros();
    let profile = execution_profile_id(plan.policy);
    check_window(
        profile,
        pin,
        &response.generation,
        &response.window,
        cap,
        response.results.len(),
    )?;
    if response.rank_unit != rank_unit {
        return Err(BenchError::Protocol(format!(
            "{profile}: lexical comparator returned the wrong rank unit"
        )));
    }
    Ok(LexicalCapture {
        response,
        query_micros,
    })
}

fn check_pool(
    name: &str,
    pin: &GenerationPin,
    rows: &[LexicalCandidate],
    expected: [u8; 32],
) -> BenchResult<()> {
    let digest = hybrid_file_candidate_pool_sha256_v1(pin, rows)
        .map_err(|error| BenchError::Protocol(format!("{name}: {error}")))?;
    if digest != expected {
        return Err(BenchError::Protocol(format!(
            "{name}: ordered candidate pool disagrees with HybridFile"
        )));
    }
    Ok(())
}

pub(super) fn run(args: &Args) -> BenchResult<()> {
    reject_unknown(
        args,
        &[
            "query-pack",
            "state-root",
            "repo-id",
            "generation",
            "lane-top-k",
            "file-top-k",
            "nl-max-tokens",
            "io-timeout-secs",
            "out",
        ],
    )?;
    let (pack, pack_file_sha256) =
        load_query_pack_with_file_digest(&PathBuf::from(required(args, "query-pack")?))?;
    let lane_top_k = u32::try_from(optional_u64(args, "lane-top-k", 0)?)
        .map_err(|error| BenchError::Config(format!("lane-top-k overflow: {error}")))?;
    let file_top_k = u32::try_from(optional_u64(args, "file-top-k", 0)?)
        .map_err(|error| BenchError::Config(format!("file-top-k overflow: {error}")))?;
    let lane_top_k =
        validate_public_top_k(lane_top_k).map_err(|error| BenchError::Config(error.to_string()))?;
    let file_top_k =
        validate_public_top_k(file_top_k).map_err(|error| BenchError::Config(error.to_string()))?;
    if lane_top_k != pack.contract_top_k || file_top_k > lane_top_k {
        return Err(BenchError::Config(
            "file-profile requires pack chunk cap equal --lane-top-k and file cap <= lane cap"
                .into(),
        ));
    }
    let state = PathBuf::from(required(args, "state-root")?);
    let repo_id = RepoId::new(required(args, "repo-id")?)
        .map_err(|error| BenchError::Config(error.to_string()))?;
    let revision_id = RevisionId::new(pack.repository_commit.as_str())
        .map_err(|error| BenchError::Config(error.to_string()))?;
    let generation = required(args, "generation")?
        .parse::<u64>()
        .map_err(|error| BenchError::Config(format!("invalid generation: {error}")))?;
    if generation == 0 {
        return Err(BenchError::Config("expected positive generation".into()));
    }
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
    let client = QuantaIndex::connect_legacy_query_only(
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
            let sdk_error = |error: quanta_index_sdk::SdkError| BenchError::Sdk(error.to_string());
            let lexical = capture_lexical(&client, &pin, &plan, lane_top_k, TextRankUnit::Chunk)?;
            let start = Instant::now();
            let canonical_request = SemanticWorkBoundedQueryRequestV1 {
                query: SemanticQueryRequest {
                    query_text: plan.semantic_text.clone(),
                    constraints: QueryConstraintSetV1::unconstrained(),
                    generation: Some(pin.clone()),
                    generation_selector: None,
                    lexical_scope: None,
                    top_k: lane_top_k,
                },
                max_work_units: SEMANTIC_WORK_OPERATIONAL_CAP_V1,
                canonical_ties: true,
            };
            let semantic = client
                .semantic()
                .query_work_bounded_v1(canonical_request.clone())
                .map_err(sdk_error)?;
            let semantic_micros = start.elapsed().as_micros();
            check_window("semantic", &pin, &semantic.query.generation, &semantic.query.window, lane_top_k, semantic.query.results.len())?;
            if !semantic.canonical_ties
                || semantic.charged_work_units == 0
                || semantic.charged_work_units > canonical_request.max_work_units
            {
                return Err(BenchError::Protocol(
                    "semantic canonical raw pool work settlement is invalid".into(),
                ));
            }
            let start = Instant::now();
            let hybrid = client
                .search()
                .hybrid()
                .native(&plan.lexical_request)
                .semantic_text(&plan.semantic_text)
                .pinned(pin.clone())
                .top_k(lane_top_k)
                .execute()
                .map_err(sdk_error)?;
            let hybrid_micros = start.elapsed().as_micros();
            check_window("hybrid", &pin, &hybrid.generation, &hybrid.window, lane_top_k, hybrid.results.len())?;
            let start = Instant::now();
            let files = client
                .search()
                .hybrid()
                .native(&plan.lexical_request)
                .semantic_text(&plan.semantic_text)
                .pinned(pin.clone())
                .top_k(lane_top_k)
                .files(file_top_k)
                .map_err(sdk_error)?;
            let file_micros = start.elapsed().as_micros();
            check_window("file.lexical", &pin, &files.generation, &files.lexical_window, lane_top_k, lexical.response.results.len())?;
            check_window("file.dense", &pin, &files.generation, &files.dense_window, lane_top_k, semantic.query.results.len())?;
            check_pool("file.lexical", &pin, &lexical.response.results, files.lexical_pool_sha256)?;
            check_pool("file.dense", &pin, &semantic.query.results, files.dense_pool_sha256)?;
            if files.results.len() > usize::try_from(file_top_k).map_or(usize::MAX, |cap| cap) {
                return Err(BenchError::Protocol("file result cap exceeded".into()));
            }
            let native_file_plan = plan_query(QueryInputPolicy::NaturalLanguageFile, &task.query, &config)
                .map_err(|error| BenchError::Config(error.to_string()))?;
            let lexical_file = capture_lexical(
                &client, &pin, &native_file_plan, file_top_k, TextRankUnit::File,
            )?;
            Ok(json!({
                "task_id": task.task_id,
                "query_identity": {
                    "original_query_sha256": plan.original_query_sha256,
                    "effective_lexical_request_sha256": plan.effective_lexical_request_sha256,
                    "semantic_text_sha256": plan.semantic_text_sha256,
                    "generic_semantic_representation_policy": GENERIC_SEMANTIC_REPRESENTATION_POLICY,
                },
                "lexical": lexical.evidence(&plan, lane_top_k),
                "lexical_file_baseline": lexical_file.evidence(&native_file_plan, file_top_k),
                "semantic": {"query_micros": semantic_micros,
                    "raw_pool_contract": "sealed_exact_canonical_ties_before_top_k_v1",
                    "canonical_ties": semantic.canonical_ties,
                    "max_work_units": canonical_request.max_work_units,
                    "charged_work_units": semantic.charged_work_units,
                    "window": semantic.query.window,
                    "candidates": semantic.query.results.iter().map(chunk).collect::<Vec<_>>()},
                "hybrid": {"query_micros": hybrid_micros, "window": hybrid.window, "candidates": hybrid.results.iter().map(|hit| json!({
                    "candidate_id": hit.candidate.candidate_id,
                    "source_repo_id": hit.candidate.source_repo_id,
                    "path": hit.candidate.repo_relative_path,
                    "score": hit.fused_score,
                    "contributions": hit.contributions,
                })).collect::<Vec<_>>()},
                "files": {"query_micros": file_micros,
                    "file_top_k": files.file_top_k,
                    "bounded_file_count": files.bounded_file_count,
                    "lexical_pool_sha256": files.lexical_pool_sha256,
                    "dense_pool_sha256": files.dense_pool_sha256,
                    "lexical_window": files.lexical_window,
                    "dense_window": files.dense_window,
                    "results": files.results.iter().map(|hit| json!({
                        "file": hit.file,
                        "source_repo_id": hit.file.source_repo_id,
                        "path": hit.file.repo_relative_path,
                        "representative_candidate_id": hit.representative().map(|row| row.candidate_id.as_str()),
                        "file_rrf_score": hit.file_rrf_score,
                        "lexical": hit.lexical.as_ref(),
                        "dense": hit.dense.as_ref(),
                    })).collect::<Vec<_>>(),
                },
            }))
        })
        .collect::<BenchResult<Vec<_>>>()?;
    let out = PathBuf::from(required(args, "out")?);
    let _digest = write_json_bound(
        &out,
        &json!({
            "schema_version": 1,
            "scope": "sealed-generation-generic-file-sdk-diagnostic",
            "generic_execution_profile_id": execution_profile_id(QueryInputPolicy::NaturalLanguage),
            "pack_sha256": pack.pack_sha256,
            "pack_file_sha256": pack_file_sha256,
            "lane_top_k": lane_top_k,
            "file_top_k": file_top_k,
            "nl_max_tokens": config.max_tokens,
            "generation": pin,
            "tasks": tasks,
        }),
    )?;
    Ok(())
}
