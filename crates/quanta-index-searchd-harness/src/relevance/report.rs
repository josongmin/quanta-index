//! Relevance rail run engine: seed → query → score → gate → artifact.
//!
//! Publishes sealed generations through the SDK to pinned Legacy processes, runs each
//! [`JudgedQuery`] against the live ranker, scores the produced ordering with the
//! pure [`super::metrics`], and enforces two blocking layers that the
//! `MEASUREMENT_MATRIX` keeps distinct:
//!
//! 1. **ordering invariants** — top-1 / top-k-containment / hard-negative
//!    exclusion, so a demoted best answer cannot hide behind a healthy average;
//! 2. **per-route metric thresholds** — `MRR@10` / `NDCG@10` / `Recall@20`.
//!
//! Artifacts land under `artifacts/search-quality/relevance/latest/`. The
//! Sourcegraph lexical overlap floor (J7Q-01B) is emitted as `unprovisioned`
//! until a local Sourcegraph instance exists — a tracked gap, never a silent
//! pass.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::Result as AnyResult;
use quanta_index_contract::{SearchCorpusActiveHeadV1, TextQuerySyntax};
use quanta_index_embed::OpenAiEmbedStatsSnapshot;
use quanta_index_searchd::app::SemanticEmbedderProfile;
use quanta_index_searchd::app::config::ProviderWorkBudgetConfig;
use serde_json::{Value, json};

use crate::artifact::{
    BenchArtifactV1, BenchMode, BenchProvenanceV1, BenchRowV1, BenchSyntax, GitHeadV1, HostV1,
    PhaseDurationsV1, ResourceUsageV1, ResultShape, RouteFamily, config_digest, corpus_digest,
    framed_digest, model_revision_of, saturating_u64,
};
use crate::legacy_reference::RelevanceProviderInputs;
#[path = "process.rs"]
mod process;

/// The artifact dimension the relevance rail writes.
pub const DIMENSION: &str = "relevance";
/// The artifact dimension the `OpenAI` A/B capture writes.
pub const OPENAI_AB_DIMENSION: &str = "relevance-openai-ab";
use crate::relevance::corpus::{
    JUDGED_QUERIES, JudgedQuery, LEXICAL_RELEVANCE_CORPUS, RelevanceRoute, SEMANTIC_GATED_QUERIES,
    SEMANTIC_JUDGED_QUERIES, SOURCEGRAPH_OVERLAP_BUCKETS, SemanticIntentKind, SemanticJudgedQuery,
    TOP_K, semantic_fixture_docs,
};
use crate::relevance::metrics::{
    GradedDoc, grade_index, ndcg_at_k, recall_at_k, reciprocal_rank_at_k,
};

/// `k` cutoff for `MRR` and `NDCG`.
const RANK_K: usize = 10;
/// `k` cutoff for `Recall`.
const RECALL_K: usize = 20;

/// Per-route blocking thresholds.
///
/// Calibrated to the construct-by-seed intent, not to current engine output: a
/// correct lexical ranker puts the definition site at rank 1 (`MRR = 1.0`),
/// keeps graded order largely intact (`NDCG` high), and retrieves every
/// relevant doc (`Recall = 1.0`). Thresholds sit below the ideal to tolerate
/// benign tie ordering while still tripping on a real demotion.
#[derive(Clone, Copy, Debug)]
#[expect(
    clippy::struct_field_names,
    reason = "the `min_` prefix is load-bearing: each field is a minimum threshold, not the measured value"
)]
pub struct RouteThresholds {
    pub min_mrr_at_10: f64,
    pub min_ndcg_at_10: f64,
    pub min_recall_at_20: f64,
}

fn thresholds_for(route: RelevanceRoute) -> RouteThresholds {
    match route {
        RelevanceRoute::Lexical => RouteThresholds {
            min_mrr_at_10: 0.99,
            min_ndcg_at_10: 0.85,
            min_recall_at_20: 1.0,
        },
        // Honesty (RFC §5 P1-3 / §7): the deterministic `Hash` embedder cannot
        // be claimed to achieve neural semantic quality, so the CI semantic
        // route is gated only on what hash genuinely owns — exact-token recall
        // (`Recall@20 = 1.0`). MRR/NDCG floors are 0.0 because head-rank ordering
        // among same-grade exact-token hits is NOT a hash property; neural-quality
        // separation lives in the OpenAI-gated discriminative subset, not here.
        RelevanceRoute::Semantic => RouteThresholds {
            min_mrr_at_10: 0.0,
            min_ndcg_at_10: 0.0,
            min_recall_at_20: 1.0,
        },
    }
}

/// Scored outcome for one judged query.
#[derive(Clone, Debug)]
pub struct QueryScore {
    pub id: &'static str,
    pub route: RelevanceRoute,
    pub intent: &'static str,
    pub query: &'static str,
    pub produced_order: Vec<String>,
    pub mrr_at_10: f64,
    pub ndcg_at_10: f64,
    pub recall_at_20: f64,
    pub failures: Vec<String>,
}

impl QueryScore {
    #[must_use]
    pub fn passed(&self) -> bool {
        self.failures.is_empty()
    }
}

/// Aggregated per-route-family report bucket.
#[derive(Clone, Debug)]
pub struct RouteSummary {
    pub route: RelevanceRoute,
    pub query_count: usize,
    pub mean_mrr_at_10: f64,
    pub mean_ndcg_at_10: f64,
    pub mean_recall_at_20: f64,
    pub passed: bool,
}

/// One captured Sourcegraph overlap bucket (J7Q-01B).
///
/// The quanta-index half is captured live; the Sourcegraph half is
/// `unprovisioned`, so `verdict` is always `unprovisioned` until a local
/// instance fills the Sourcegraph ordering. A capture error on the quanta side
/// is recorded as-is (never hidden) but does NOT gate the relevance rail — the
/// overlap is a tracked external side lane, not a blocking metric.
#[derive(Clone, Debug)]
pub struct OverlapCapture {
    pub bucket: &'static str,
    pub query_family: &'static str,
    pub query: &'static str,
    pub syntax: BenchSyntax,
    pub sourcegraph_surface: &'static str,
    /// Live quanta-index ordering (doc-id list, top-first); empty on capture error.
    pub quanta_ordering: Vec<String>,
    /// The typed error the quanta query produced, if any (recorded, not hidden).
    pub quanta_capture_error: Option<String>,
}

/// The full relevance report.
#[derive(Clone, Debug)]
pub struct RelevanceReport {
    pub queries: Vec<QueryScore>,
    pub routes: Vec<RouteSummary>,
    pub overlap: Vec<OverlapCapture>,
    pub passed: bool,
    /// The embedder the semantic gate's fixture was built under, for the
    /// provenance.
    pub model_revision: Option<String>,
    /// Model-backed paraphrase gate; absent only on the explicit hash-dev rail.
    pub semantic_quality: Option<SemanticQualitySummary>,
    /// Peak resident bytes of the external daemon that served this run.
    pub peak_daemon_rss_bytes: u64,
    /// Pinned Legacy executable used by both fixture processes.
    pub legacy_daemon_sha256: String,
}

const MIN_PARAPHRASE_MRR_AT_10: f64 = 0.65;
const MIN_PARAPHRASE_NDCG_AT_10: f64 = 0.70;
const MIN_PARAPHRASE_TOP1_RATE: f64 = 0.50;
const REQUIRED_PARAPHRASE_CASES: usize = 12;

#[derive(Clone, Debug)]
pub struct SemanticQualitySummary {
    pub case_count: usize,
    pub top1_count: usize,
    pub mean_mrr_at_10: f64,
    pub mean_ndcg_at_10: f64,
    pub mean_recall_at_20: f64,
    pub passed: bool,
}

fn summarize_semantic_quality(cases: &[SemanticCaseMetrics]) -> SemanticQualitySummary {
    let count = crate::relevance::metrics::usize_to_f64(cases.len());
    let mean = |metric: &dyn Fn(&SemanticCaseMetrics) -> f64| -> f64 {
        if cases.is_empty() {
            0.0
        } else {
            cases.iter().map(metric).sum::<f64>() / count
        }
    };
    let top1_count = cases.iter().filter(|case| case.top1_is_on_topic).count();
    let mean_mrr_at_10 = mean(&|case| case.mrr_at_10);
    let mean_ndcg_at_10 = mean(&|case| case.ndcg_at_10);
    let mean_recall_at_20 = mean(&|case| case.recall_at_20);
    SemanticQualitySummary {
        case_count: cases.len(),
        top1_count,
        mean_mrr_at_10,
        mean_ndcg_at_10,
        mean_recall_at_20,
        passed: cases.len() == REQUIRED_PARAPHRASE_CASES
            && mean_mrr_at_10 >= MIN_PARAPHRASE_MRR_AT_10
            && mean_ndcg_at_10 >= MIN_PARAPHRASE_NDCG_AT_10
            && crate::relevance::metrics::usize_to_f64(top1_count) / count
                >= MIN_PARAPHRASE_TOP1_RATE,
    }
}

fn to_text_syntax(syntax: BenchSyntax) -> TextQuerySyntax {
    match syntax {
        BenchSyntax::Native => TextQuerySyntax::Native,
        BenchSyntax::Sourcegraph => TextQuerySyntax::Sourcegraph,
    }
}

/// The judged semantic fixture's exact file/chunk boundaries for both producers.
#[derive(Clone, Copy)]
struct SemanticFixturePiece {
    text: &'static str,
    start_line: u32,
    end_line: u32,
}

fn semantic_fixture_files() -> Vec<(&'static str, Vec<SemanticFixturePiece>)> {
    semantic_fixture_docs()
        .into_iter()
        .map(|(path, content)| {
            let pieces = if path == "auth/token_refresh.rs" {
                let (head, tail) = content.split_at(content.len().div_euclid(2));
                vec![
                    SemanticFixturePiece {
                        text: head,
                        start_line: 1,
                        end_line: 2,
                    },
                    SemanticFixturePiece {
                        text: tail,
                        start_line: 3,
                        end_line: 4,
                    },
                ]
            } else {
                vec![SemanticFixturePiece {
                    text: content,
                    start_line: 1,
                    end_line: 2,
                }]
            };
            (path, pieces)
        })
        .collect()
}

/// Collapse repeated same-repo same-path hits to the earliest occurrence,
/// preserving the produced order of first occurrences.
///
/// The semantic route is scored at file granularity, but the engine returns one
/// candidate per CHUNK, so a single file can appear several times. Collapsing to
/// the first occurrence (RFC §5 P1-2 / §8) keeps one file from occupying
/// multiple head ranks and from inflating or obscuring file-level metrics. This
/// is deterministic: given the same produced order it returns the same collapsed
/// order.
fn collapse_same_repo_path(repo_paths: Vec<(String, String)>) -> Vec<String> {
    let mut seen = std::collections::BTreeSet::new();
    let mut collapsed = Vec::with_capacity(repo_paths.len());
    for (repo_id, path) in repo_paths {
        if seen.insert((repo_id, path.clone())) {
            collapsed.push(path);
        }
    }
    collapsed
}

/// Enforce the ordering invariants for one query, collecting every violation.
fn check_invariants(query: &JudgedQuery, order: &[String]) -> Vec<String> {
    let mut failures = Vec::new();
    let rank_of = |doc: &str| order.iter().position(|d| d == doc);

    if let Some(top1) = query.ordering.top1 {
        match order.first() {
            Some(first) if first == top1 => {}
            Some(first) => failures.push(format!(
                "top1 invariant: expected `{top1}` at rank 1, got `{first}`"
            )),
            None => failures.push(format!(
                "top1 invariant: expected `{top1}` at rank 1, got empty ranking"
            )),
        }
    }

    for required in query.ordering.top_k_contains {
        if rank_of(required).is_none() {
            failures.push(format!(
                "top_k_contains invariant: `{required}` missing from produced top-{TOP_K}"
            ));
        }
    }

    for (doc, max_rank_exclusive) in query.ordering.forbidden_within {
        if let Some(idx) = rank_of(doc) {
            let rank = idx.saturating_add(1);
            if rank < *max_rank_exclusive {
                failures.push(format!(
                    "forbidden_within invariant: hard negative `{doc}` reached rank {rank} (must be >= {max_rank_exclusive})"
                ));
            }
        }
    }
    failures
}

/// Score one judged query end to end (metrics + invariants + thresholds).
fn score_query(query: &JudgedQuery, order: Vec<String>) -> AnyResult<QueryScore> {
    let judged: Vec<GradedDoc> = query
        .judgments
        .iter()
        .map(|(doc, grade)| GradedDoc {
            doc_id: (*doc).to_string(),
            grade: *grade,
        })
        .collect();
    let grades: BTreeMap<&str, u8> = grade_index(&judged)?;

    let mrr = reciprocal_rank_at_k(&order, &grades, RANK_K);
    let ndcg = ndcg_at_k(&order, &grades, RANK_K);
    let recall = recall_at_k(&order, &grades, RECALL_K);

    let mut failures = check_invariants(query, &order);
    let thresholds = thresholds_for(query.route);
    if mrr < thresholds.min_mrr_at_10 {
        failures.push(format!(
            "MRR@10 {mrr:.4} below threshold {:.4}",
            thresholds.min_mrr_at_10
        ));
    }
    if ndcg < thresholds.min_ndcg_at_10 {
        failures.push(format!(
            "NDCG@10 {ndcg:.4} below threshold {:.4}",
            thresholds.min_ndcg_at_10
        ));
    }
    if recall < thresholds.min_recall_at_20 {
        failures.push(format!(
            "Recall@20 {recall:.4} below threshold {:.4}",
            thresholds.min_recall_at_20
        ));
    }

    Ok(QueryScore {
        id: query.id,
        route: query.route,
        intent: query.intent,
        query: query.query,
        produced_order: order,
        mrr_at_10: mrr,
        ndcg_at_10: ndcg,
        recall_at_20: recall,
        failures,
    })
}

fn aggregate_routes(queries: &[QueryScore]) -> Vec<RouteSummary> {
    let mut by_route: BTreeMap<&'static str, (RelevanceRoute, Vec<&QueryScore>)> = BTreeMap::new();
    for score in queries {
        by_route
            .entry(score.route.as_str())
            .or_insert_with(|| (score.route, Vec::new()))
            .1
            .push(score);
    }
    by_route
        .into_values()
        .map(|(route, scores)| {
            let n = crate::relevance::metrics::usize_to_f64(scores.len());
            let mean = |f: &dyn Fn(&QueryScore) -> f64| -> f64 {
                if scores.is_empty() {
                    return 0.0;
                }
                scores.iter().map(|s| f(s)).sum::<f64>() / n
            };
            RouteSummary {
                route,
                query_count: scores.len(),
                mean_mrr_at_10: mean(&|s| s.mrr_at_10),
                mean_ndcg_at_10: mean(&|s| s.ndcg_at_10),
                mean_recall_at_20: mean(&|s| s.recall_at_20),
                passed: scores.iter().all(|s| s.passed()),
            }
        })
        .collect()
}

/// Run the production-default relevance rail against fresh pinned processes.
pub fn run_relevance_report() -> AnyResult<RelevanceReport> {
    run_relevance_report_with_profile(SemanticEmbedderProfile::from_env()?)
}

/// Run the same judged corpus under an explicitly resolved query/corpus model.
pub fn run_relevance_report_with_profile(
    profile: SemanticEmbedderProfile,
) -> AnyResult<RelevanceReport> {
    process::run(profile)
}

/// The bytes both relevance fixtures ingest, in ingest order: the lexical
/// judged corpus, then the semantic corpus with its distractors.
#[must_use]
pub fn relevance_corpus_files() -> Vec<(String, String)> {
    LEXICAL_RELEVANCE_CORPUS
        .iter()
        .chain(semantic_fixture_docs().iter())
        .map(|(path, content)| ((*path).to_string(), (*content).to_string()))
        .collect()
}

#[derive(Clone, Debug)]
pub struct SemanticCaseMetrics {
    pub produced_order: Vec<String>,
    pub mrr_at_10: f64,
    pub ndcg_at_10: f64,
    pub recall_at_20: f64,
    pub top1_is_on_topic: bool,
    pub hard_negative_rank: Option<usize>,
    /// 1-based rank of the on-topic file in the produced order (`None` if absent).
    /// This is the discriminative A/B signal: `recall@20` saturates to 1.0 once
    /// the corpus has <= 20 docs (every doc is always retrieved), so the rank of
    /// the on-topic file — not recall — is what separates neural from hash.
    pub on_topic_rank: Option<usize>,
}

#[derive(Clone, Debug)]
pub struct SemanticAbCase {
    pub id: &'static str,
    pub intent_kind: SemanticIntentKind,
    pub intent: &'static str,
    pub query: &'static str,
    pub on_topic_path: &'static str,
    pub hard_negative_path: &'static str,
    pub hash: SemanticCaseMetrics,
    pub openai: SemanticCaseMetrics,
}

impl SemanticAbCase {
    fn openai_ranks_on_topic_higher(&self) -> bool {
        match (self.openai.on_topic_rank, self.hash.on_topic_rank) {
            (Some(openai_rank), Some(hash_rank)) => openai_rank < hash_rank,
            (Some(_), None) => true,
            _ => false,
        }
    }
}

#[derive(Clone, Debug)]
pub struct OpenAiSemanticAbReport {
    pub cases: Vec<SemanticAbCase>,
    provider_work_budget: ProviderWorkBudgetConfig,
    pub provider_stats: OpenAiEmbedStatsSnapshot,
    /// Effective process inputs with credentials omitted.
    runner_config: Value,
    model_revision: String,
    source_evidence: OpenAiAbSourceEvidence,
    peak_daemon_rss_bytes: u64,
}

#[derive(Clone, Debug, serde::Serialize)]
struct OpenAiAbSourceEvidence {
    legacy_daemon_sha256: String,
    source_batch_digest: String,
    hash: SearchCorpusActiveHeadV1,
    openai: SearchCorpusActiveHeadV1,
    openai_publication_http_requests: u64,
    openai_publication_stats: Value,
}

fn score_semantic_case(
    query: &SemanticJudgedQuery,
    produced_order: Vec<String>,
) -> AnyResult<SemanticCaseMetrics> {
    let judged = vec![
        GradedDoc {
            doc_id: query.on_topic_path.to_string(),
            grade: 3,
        },
        GradedDoc {
            doc_id: query.hard_negative_path.to_string(),
            grade: 0,
        },
    ];
    let grades = grade_index(&judged)?;
    Ok(SemanticCaseMetrics {
        mrr_at_10: reciprocal_rank_at_k(&produced_order, &grades, RANK_K),
        ndcg_at_10: ndcg_at_k(&produced_order, &grades, RANK_K),
        recall_at_20: recall_at_k(&produced_order, &grades, RECALL_K),
        top1_is_on_topic: produced_order.first().map(String::as_str) == Some(query.on_topic_path),
        hard_negative_rank: produced_order
            .iter()
            .position(|doc| doc == query.hard_negative_path)
            .map(|index| index.saturating_add(1)),
        on_topic_rank: produced_order
            .iter()
            .position(|doc| doc == query.on_topic_path)
            .map(|index| index.saturating_add(1)),
        produced_order,
    })
}

pub fn openai_profile_from_env_for_relevance_ab() -> AnyResult<SemanticEmbedderProfile> {
    match SemanticEmbedderProfile::from_env()? {
        profile @ SemanticEmbedderProfile::OpenAi { .. } => Ok(profile),
        other @ (SemanticEmbedderProfile::Hash { .. }
        | SemanticEmbedderProfile::PotionCode { .. }
        | SemanticEmbedderProfile::LocalCode { .. }
        | SemanticEmbedderProfile::Unavailable) => Err(anyhow::anyhow!(
            "local OpenAI A/B requires an OpenAi semantic profile; got {other:?}"
        )),
    }
}

const APPROVED_AB_INPUT_DIGEST_ENV: &str = "QUANTA_INDEX_OPENAI_AB_APPROVED_INPUT_DIGEST";

struct OpenAiAbInputs {
    hash_profile: SemanticEmbedderProfile,
    provider: RelevanceProviderInputs,
    runner_config: Value,
    fixture: process::OpenAiAbFixture,
    config_digest: String,
    judgments_digest: String,
    approval_digest: String,
}

fn prepare_openai_ab_inputs(openai_profile: &SemanticEmbedderProfile) -> AnyResult<OpenAiAbInputs> {
    let SemanticEmbedderProfile::OpenAi {
        model,
        model_revision,
        dimension,
        tuning,
        ..
    } = openai_profile
    else {
        anyhow::bail!("the OpenAI A/B arm requires an OpenAI semantic profile");
    };
    let openai_inputs = RelevanceProviderInputs::from_env()?;
    let grant = &openai_inputs.grant;
    anyhow::ensure!(
        grant.source_content_consent,
        "OpenAI A/B requires explicit provider Source content consent"
    );
    let config = tuning.provider_config(
        model.clone(),
        model_revision.clone(),
        *dimension,
        String::new(),
    );
    let (model_id, revision, ..) = config.model_metadata()?;
    config.validate_approved_egress(&grant.to_grant("openai", &model_id, revision))?;
    let request_cost = openai_inputs
        .budget
        .openai_max_request_cost_micros
        .ok_or_else(|| {
            anyhow::anyhow!(
                "OpenAI A/B requires QUANTA_INDEX_PROVIDER_OPENAI_MAX_REQUEST_COST_MICROS"
            )
        })?;
    anyhow::ensure!(
        std::env::var("QUANTA_INDEX_PROVIDER_COST_CEILING_MICROS")
            .is_ok_and(|value| !value.trim().is_empty())
            && request_cost.get() <= openai_inputs.budget.total_cost_micros_ceiling,
        "OpenAI A/B requires an explicit positive cost ceiling funding the approved request cost"
    );
    let provider_egress_grant_digest = config_digest(
        "relevance-openai-ab-provider-grant",
        &[
            ("tenant", grant.tenant_id.clone()),
            ("endpoint", grant.endpoint.clone()),
            ("region", grant.region.clone()),
            ("retention", grant.retention.clone()),
            ("profile", grant.profile.clone()),
            (
                "source_content_consent",
                grant.source_content_consent.to_string(),
            ),
        ],
    );
    let hash_profile = SemanticEmbedderProfile::hash_dev();
    let runner_config = openai_ab_runner_config_json(
        &hash_profile,
        openai_profile,
        &openai_inputs.budget,
        &provider_egress_grant_digest,
    )?;
    let fixture = process::OpenAiAbFixture::prepare()?;
    let config_digest = openai_ab_config_digest(&runner_config);
    let judgments_digest = openai_ab_judgments_digest();
    let approval_digest = openai_ab_input_digest(
        &config_digest,
        &fixture.source_batch_digest(),
        &judgments_digest,
    );
    Ok(OpenAiAbInputs {
        hash_profile,
        provider: openai_inputs,
        runner_config,
        fixture,
        config_digest,
        judgments_digest,
        approval_digest,
    })
}

fn openai_ab_judgments_digest() -> String {
    let parts = SEMANTIC_JUDGED_QUERIES
        .iter()
        .flat_map(|case| {
            [
                case.id,
                semantic_intent_kind_label(case.intent_kind),
                case.intent,
                case.query,
                case.on_topic_path,
                case.hard_negative_path,
            ]
        })
        .map(str::as_bytes)
        .collect::<Vec<_>>();
    framed_digest(
        "quanta-index:bench:relevance-openai-ab:judgments:v1",
        &parts,
    )
}

fn openai_ab_input_digest(
    config_digest_value: &str,
    source_batch_digest: &str,
    judgments_digest: &str,
) -> String {
    config_digest(
        "relevance-openai-ab-inputs",
        &[
            ("config_digest", config_digest_value.to_owned()),
            ("source_batch_digest", source_batch_digest.to_owned()),
            ("judgments_digest", judgments_digest.to_owned()),
        ],
    )
}

/// Resolve the capture inputs for operator review without launching either arm.
pub fn describe_openai_ab_inputs(openai_profile: &SemanticEmbedderProfile) -> AnyResult<Value> {
    let inputs = prepare_openai_ab_inputs(openai_profile)?;
    Ok(json!({
        "runner_config": inputs.runner_config,
        "config_digest": inputs.config_digest,
        "source_batch_digest": inputs.fixture.source_batch_digest(),
        "judgments_digest": inputs.judgments_digest,
        "approval_digest": inputs.approval_digest,
    }))
}

pub fn run_openai_semantic_ab_report(
    openai_profile: SemanticEmbedderProfile,
) -> AnyResult<OpenAiSemanticAbReport> {
    let inputs = prepare_openai_ab_inputs(&openai_profile)?;
    anyhow::ensure!(
        std::env::var(APPROVED_AB_INPUT_DIGEST_ENV).as_deref()
            == Ok(inputs.approval_digest.as_str()),
        "OpenAI A/B requires {APPROVED_AB_INPUT_DIGEST_ENV} matching the reviewed config, Source batch and judgments"
    );
    let model_revision = model_revision_of(&openai_profile)
        .ok_or_else(|| anyhow::anyhow!("OpenAI A/B requires a model revision"))?;
    let capture = process::run_openai_ab(
        &inputs.hash_profile,
        &openai_profile,
        &inputs.provider,
        &inputs.fixture,
    )?;

    Ok(OpenAiSemanticAbReport {
        cases: capture.cases,
        provider_work_budget: inputs.provider.budget,
        provider_stats: capture.provider_stats,
        runner_config: inputs.runner_config,
        model_revision,
        source_evidence: capture.source_evidence,
        peak_daemon_rss_bytes: capture.peak_daemon_rss_bytes,
    })
}

// ---------------------------------------------------------------------------
// Artifact emission.
// ---------------------------------------------------------------------------

fn query_score_json(score: &QueryScore) -> Value {
    let thresholds = thresholds_for(score.route);
    json!({
        "id": score.id,
        "route": score.route.as_str(),
        "intent": score.intent,
        "query": score.query,
        "produced_order": score.produced_order,
        "mrr_at_10": score.mrr_at_10,
        "ndcg_at_10": score.ndcg_at_10,
        "recall_at_20": score.recall_at_20,
        "thresholds": {
            "min_mrr_at_10": thresholds.min_mrr_at_10,
            "min_ndcg_at_10": thresholds.min_ndcg_at_10,
            "min_recall_at_20": thresholds.min_recall_at_20,
        },
        "failures": score.failures,
        "passed": score.passed(),
    })
}

fn route_summary_json(summary: &RouteSummary) -> Value {
    json!({
        "route": summary.route.as_str(),
        "query_count": summary.query_count,
        "mean_mrr_at_10": summary.mean_mrr_at_10,
        "mean_ndcg_at_10": summary.mean_ndcg_at_10,
        "mean_recall_at_20": summary.mean_recall_at_20,
        "passed": summary.passed,
    })
}

/// Judgment manifest mirror — the checked-in SSOT, persisted alongside results
/// so a reviewer can diff intended grades against produced order in one place.
fn judgments_json(report: &RelevanceReport) -> AnyResult<Value> {
    let mut rows: Vec<Value> = JUDGED_QUERIES
        .iter()
        .chain(SEMANTIC_GATED_QUERIES.iter())
        .map(|q| -> AnyResult<Value> {
            let judgments: Vec<Value> = q
                .judgments
                .iter()
                .map(|(doc, grade)| json!({ "doc": doc, "grade": grade }))
                .collect();
            let produced = report
                .queries
                .iter()
                .find(|s| s.id == q.id)
                .map(|s| s.produced_order.clone())
                .ok_or_else(|| anyhow::anyhow!("missing produced ranking for {}", q.id))?;
            Ok(json!({
                "id": q.id,
                "route": q.route.as_str(),
                "intent": q.intent,
                "query": q.query,
                "judgments": judgments,
                "ordering_invariants": {
                    "top1": q.ordering.top1,
                    "top_k_contains": q.ordering.top_k_contains,
                    "forbidden_within": q.ordering.forbidden_within
                        .iter()
                        .map(|(doc, rank)| json!({ "doc": doc, "max_rank_exclusive": rank }))
                        .collect::<Vec<_>>(),
                },
                "produced_order": produced,
            }))
        })
        .collect::<AnyResult<Vec<_>>>()?;
    if report.semantic_quality.is_some() {
        let paraphrase_rows = SEMANTIC_JUDGED_QUERIES
            .iter()
            .filter(|query| query.intent_kind == SemanticIntentKind::Paraphrase)
            .map(|query| -> AnyResult<Value> {
                let produced = report
                    .queries
                    .iter()
                    .find(|score| score.id == query.id)
                    .map(|score| score.produced_order.clone())
                    .ok_or_else(|| anyhow::anyhow!("missing produced ranking for {}", query.id))?;
                Ok(json!({
                    "id": query.id,
                    "route": "semantic",
                    "intent": query.intent,
                    "query": query.query,
                    "intent_kind": "paraphrase",
                    "judgments": [
                        {"doc": query.on_topic_path, "grade": 3},
                        {"doc": query.hard_negative_path, "grade": 0},
                    ],
                    "produced_order": produced,
                }))
            })
            .collect::<AnyResult<Vec<_>>>()?;
        rows.extend(paraphrase_rows);
    }
    Ok(json!({
        "kind": "quanta-index-relevance-judgments",
        "supplement_schema_version": 1,
        "queries": rows,
    }))
}

/// The relevance artifact's detail: every route and query verdict and the
/// external floor's status.
#[must_use]
pub fn detail_json(report: &RelevanceReport) -> Value {
    json!({
        "backend": "legacy-reference",
        "passed": report.passed,
        "legacy_daemon_sha256": report.legacy_daemon_sha256,
        "resource_scope": "peak RSS of pinned Legacy daemon processes",
        "semantic_quality": report.semantic_quality.as_ref().map(|quality| json!({
            "case_count": quality.case_count,
            "top1_count": quality.top1_count,
            "mean_mrr_at_10": quality.mean_mrr_at_10,
            "mean_ndcg_at_10": quality.mean_ndcg_at_10,
            "mean_recall_at_20": quality.mean_recall_at_20,
            "minimum_mean_mrr_at_10": MIN_PARAPHRASE_MRR_AT_10,
            "minimum_mean_ndcg_at_10": MIN_PARAPHRASE_NDCG_AT_10,
            "minimum_top1_rate": MIN_PARAPHRASE_TOP1_RATE,
            "passed": quality.passed,
        })),
        "routes": report.routes.iter().map(route_summary_json).collect::<Vec<_>>(),
        "queries": report.queries.iter().map(query_score_json).collect::<Vec<_>>(),
        "external_lexical_floor": {
            "source": "sourcegraph-lexical-overlap",
            "status": "unprovisioned",
            "owner_ticket": "J7Q-01B",
            "note": "local Sourcegraph instance not provisioned; external lexical floor NOT met",
        },
    })
}

/// The artifact's actual retrieval route, independent of its result shape.
const fn relevance_route_family(route: RelevanceRoute) -> RouteFamily {
    match route {
        RelevanceRoute::Lexical => RouteFamily::Lexical,
        RelevanceRoute::Semantic => RouteFamily::Semantic,
    }
}

/// One artifact row per judged query: no latency (the rail measures
/// ranking, not time), the produced page's size, and a typed error count
/// of one when the query failed its gate.
fn query_row(score: &QueryScore) -> BenchRowV1 {
    BenchRowV1 {
        scenario_id: format!("relevance.{}.{}", score.route.as_str(), score.id),
        route_family: relevance_route_family(score.route),
        syntax: BenchSyntax::Native,
        result_shape: if score.produced_order.is_empty() {
            ResultShape::Empty
        } else {
            ResultShape::Candidates
        },
        latency: None,
        qps: None,
        error_count: u64::from(!score.passed()),
        timeout_count: 0,
        result_count: Some(saturating_u64(score.produced_order.len())),
        typed_error_code: None,
        engine_touched: vec![score.route.as_str().to_string()],
        early_stop_reason: None,
    }
}

/// The relevance artifact: one `BenchArtifactV1` whose rows are the judged
/// queries.
///
/// Its provenance names the head, both fixtures' bytes, the thresholds'
/// cutoffs and the embedder of the semantic gate.
pub fn artifact(
    report: &RelevanceReport,
    git_head: GitHeadV1,
    host: HostV1,
) -> AnyResult<BenchArtifactV1> {
    Ok(BenchArtifactV1 {
        dimension: DIMENSION.to_string(),
        mode: BenchMode::Warm,
        concurrency: 1,
        provenance: BenchProvenanceV1 {
            git_head,
            corpus_digest: corpus_digest(DIMENSION, &relevance_corpus_files()),
            config_digest: config_digest(
                DIMENSION,
                &[
                    ("rank_k", RANK_K.to_string()),
                    ("recall_k", RECALL_K.to_string()),
                    ("top_k", TOP_K.to_string()),
                    ("backend", "legacy-reference".to_string()),
                    ("legacy_daemon_sha256", report.legacy_daemon_sha256.clone()),
                    ("judged_queries", JUDGED_QUERIES.len().to_string()),
                    (
                        "semantic_gated_queries",
                        SEMANTIC_GATED_QUERIES.len().to_string(),
                    ),
                    (
                        "model_backed_cases",
                        report
                            .semantic_quality
                            .as_ref()
                            .map_or(0, |quality| quality.case_count)
                            .to_string(),
                    ),
                    (
                        "min_paraphrase_mrr_at_10",
                        MIN_PARAPHRASE_MRR_AT_10.to_string(),
                    ),
                    (
                        "min_paraphrase_ndcg_at_10",
                        MIN_PARAPHRASE_NDCG_AT_10.to_string(),
                    ),
                    (
                        "min_paraphrase_top1_rate",
                        MIN_PARAPHRASE_TOP1_RATE.to_string(),
                    ),
                ],
            ),
            model_revision: report.model_revision.clone(),
        },
        host,
        resources: ResourceUsageV1 {
            peak_rss_bytes: report.peak_daemon_rss_bytes,
        },
        phases: PhaseDurationsV1::default(),
        disk_amplification: None,
        rows: report.queries.iter().map(query_row).collect(),
        detail: detail_json(report),
    })
}

/// One comparison row in the Sourcegraph overlap artifact.
///
/// Carries every field the `COMMAND_AND_ARTIFACT_CONTRACT` requires per overlap
/// row: query family, exact query text, capture date, Sourcegraph surface, the
/// quanta-index ordering (live), the Sourcegraph ordering (`unprovisioned`), and
/// a pass/fail-or-gap note. `verdict` is `unprovisioned` because the Sourcegraph
/// side is absent — a half-captured row can never read as a pass.
fn overlap_comparison_json(capture: &OverlapCapture, capture_date: &str) -> Value {
    json!({
        "bucket": capture.bucket,
        "query_family": capture.query_family,
        "query": capture.query,
        "syntax": to_text_syntax(capture.syntax).as_str(),
        "capture_date": capture_date,
        "sourcegraph_surface": capture.sourcegraph_surface,
        "quanta_index_ordering": capture.quanta_ordering,
        "quanta_capture_error": capture.quanta_capture_error,
        "sourcegraph_ordering": "unprovisioned",
        "verdict": "unprovisioned",
        "gap_note": "external comparison pending local Sourcegraph provisioning; quanta-index ordering captured, Sourcegraph ordering absent",
    })
}

/// The Sourcegraph lexical overlap artifact (J7Q-01B).
///
/// The quanta-index half of every bucket is captured live and persisted; the
/// Sourcegraph half is `unprovisioned`. Recording it as a real, non-passing
/// artifact keeps the external floor honest instead of letting its absence read
/// as success — `external_floor_met` is `false` and every bucket verdict is
/// `unprovisioned`, so no competitive claim can be derived. When a local
/// Sourcegraph instance lands, only the Sourcegraph ordering + verdict need
/// filling; the buckets, query text, and quanta orderings are already proven.
fn sourcegraph_overlap_json(report: &RelevanceReport, capture_date: &str) -> Value {
    json!({
        "kind": "quanta-index-sourcegraph-overlap-supplement",
        "supplement_schema_version": 1,
        "status": "unprovisioned",
        "owner_ticket": "J7Q-01B",
        "external_floor_met": false,
        "capture_date": capture_date,
        "note": "quanta-index ordering captured live per bucket; local Sourcegraph instance not provisioned, so each bucket verdict stays unprovisioned and the external lexical floor is NOT met",
        "overlap_buckets": SOURCEGRAPH_OVERLAP_BUCKETS.iter().map(|bucket| bucket.bucket).collect::<Vec<_>>(),
        "comparisons": report
            .overlap
            .iter()
            .map(|capture| overlap_comparison_json(capture, capture_date))
            .collect::<Vec<_>>(),
    })
}

/// Write the three canonical relevance artifacts under `dir`.
///
/// `capture_date` stamps the Sourcegraph overlap rows (when the quanta-index
/// half was captured); it is injected by the caller so artifact emission stays
/// deterministic under test.
pub fn write_artifacts(
    report: &RelevanceReport,
    dir: &Path,
    git_head: GitHeadV1,
    host: HostV1,
    capture_date: &str,
) -> AnyResult<()> {
    let summary = artifact(report, git_head, host)?;
    let judgments = judgments_json(report)?;
    let overlap = sourcegraph_overlap_json(report, capture_date);
    summary.write_to(&dir.join("summary.json"))?;
    crate::artifact::write_json_pretty(&dir.join("query_judgments.json"), &judgments)?;
    crate::artifact::write_json_pretty(&dir.join("sourcegraph-overlap.json"), &overlap)?;
    Ok(())
}

fn semantic_intent_kind_label(kind: SemanticIntentKind) -> &'static str {
    match kind {
        SemanticIntentKind::ExactToken => "exact_token",
        SemanticIntentKind::Paraphrase => "paraphrase",
    }
}

fn semantic_case_metrics_json(metrics: &SemanticCaseMetrics) -> Value {
    json!({
        "produced_order": metrics.produced_order,
        "mrr_at_10": metrics.mrr_at_10,
        "ndcg_at_10": metrics.ndcg_at_10,
        "recall_at_20": metrics.recall_at_20,
        "top1_is_on_topic": metrics.top1_is_on_topic,
        "hard_negative_rank": metrics.hard_negative_rank,
        "on_topic_rank": metrics.on_topic_rank,
    })
}

fn openai_ab_cases_json(report: &OpenAiSemanticAbReport) -> Value {
    let rows: Vec<Value> = report
        .cases
        .iter()
        .map(|case| {
            json!({
                "id": case.id,
                "intent_kind": semantic_intent_kind_label(case.intent_kind),
                "intent": case.intent,
                "query": case.query,
                "on_topic_path": case.on_topic_path,
                "hard_negative_path": case.hard_negative_path,
                "hash": semantic_case_metrics_json(&case.hash),
                "openai": semantic_case_metrics_json(&case.openai),
                "delta": {
                    "mrr_at_10": case.openai.mrr_at_10 - case.hash.mrr_at_10,
                    "ndcg_at_10": case.openai.ndcg_at_10 - case.hash.ndcg_at_10,
                    // on_topic_rank is the headline (finest) signal. recall@20 is a
                    // COARSE top-20-membership delta: 0 when both keep the file in
                    // the top 20, non-zero only when one drops it out entirely (so
                    // it is vacuous on a <=20-doc corpus but does move on a larger
                    // one). Kept for completeness, never the headline.
                    "recall_at_20": case.openai.recall_at_20 - case.hash.recall_at_20,
                    "on_topic_rank_hash": case.hash.on_topic_rank,
                    "on_topic_rank_openai": case.openai.on_topic_rank,
                    "openai_ranks_on_topic_higher": case.openai_ranks_on_topic_higher(),
                    "top1_win": case.openai.top1_is_on_topic && !case.hash.top1_is_on_topic,
                },
            })
        })
        .collect();
    json!({
        "kind": "quanta-index-relevance-openai-ab-cases",
        "supplement_schema_version": 1,
        "cases": rows,
    })
}

fn openai_provider_stats_json(stats: &OpenAiEmbedStatsSnapshot) -> Value {
    // The maxima are exact over every request observed; the samples are
    // the bounded recent window (QI-BB-009), so a reader must not derive
    // the maxima from them.
    json!({
        "schema_version": 2,
        "total_texts_observed": stats.total_texts_observed,
        "cache_hits": stats.cache_hits,
        "distinct_miss_texts": stats.distinct_miss_texts,
        "http_request_count": stats.http_request_count,
        "retry_count": stats.retry_count,
        "retryable_status_count": stats.retryable_status_count,
        "transport_error_count": stats.transport_error_count,
        "max_request_texts": stats.max_request_texts,
        "max_estimated_tokens": stats.max_estimated_tokens,
        "request_samples_dropped": stats.request_samples_dropped,
        "request_samples": stats.request_samples.iter().map(|sample| {
            json!({
                "texts_submitted": sample.texts_submitted,
                "estimated_tokens": sample.estimated_tokens,
            })
        }).collect::<Vec<_>>(),
    })
}

fn provider_work_budget_json(budget: &ProviderWorkBudgetConfig) -> Value {
    json!({
        "inflight_requests_cap": budget.inflight_requests_cap,
        "inflight_bytes_cap": budget.inflight_bytes_cap,
        "total_cost_micros_ceiling": budget.total_cost_micros_ceiling,
        "retry_attempts_cap": budget.retry_attempts_cap,
        "openai_max_request_cost_micros": budget.openai_max_request_cost_micros,
    })
}

/// The effective, non-secret inputs that can change A/B ordering or provider work.
/// Both artifact detail and its configuration digest consume this same value.
fn openai_ab_runner_config_json(
    hash_profile: &SemanticEmbedderProfile,
    openai_profile: &SemanticEmbedderProfile,
    budget: &ProviderWorkBudgetConfig,
    provider_egress_grant_digest: &str,
) -> AnyResult<Value> {
    let SemanticEmbedderProfile::OpenAi {
        model,
        model_revision,
        dimension,
        tuning,
        ..
    } = openai_profile
    else {
        anyhow::bail!("the OpenAI A/B runner config requires an OpenAI semantic profile");
    };
    let hash_model_revision = model_revision_of(hash_profile);
    let SemanticEmbedderProfile::Hash {
        dimension: hash_dimension,
    } = hash_profile
    else {
        anyhow::bail!("the OpenAI A/B hash arm requires a Hash semantic profile");
    };
    let retention = tuning.cache_retention;
    Ok(json!({
        "schema_version": 1,
        "hash": {
            "dimension": hash_dimension,
            "model_revision": hash_model_revision,
        },
        "openai": {
            "model": model,
            "model_revision": model_revision,
            "dimension": dimension,
            "max_batch": tuning.max_batch,
            "max_estimated_tokens_per_request": tuning.max_estimated_tokens_per_request,
            "max_retries": tuning.max_retries,
            "timeout_secs": tuning.timeout.as_secs(),
            "timeout_subsec_nanos": tuning.timeout.subsec_nanos(),
            "cache_enabled": tuning.cache_enabled,
            "cache_retention": {
                "max_entries": retention.max_entries(),
                "max_resident_bytes": retention.max_resident_bytes(),
                "max_namespaces": retention.max_namespaces(),
                "max_entry_age_secs": retention.max_entry_age().as_secs(),
                "max_entry_age_subsec_nanos": retention.max_entry_age().subsec_nanos(),
                "max_total_bytes": retention.max_total_bytes(),
            },
            "concurrency": tuning.concurrency,
        },
        "provider_work_budget": provider_work_budget_json(budget),
        "provider_egress_grant_digest": provider_egress_grant_digest,
    }))
}

fn openai_ab_detail_json(
    report: &OpenAiSemanticAbReport,
    runner_config: Value,
) -> AnyResult<Value> {
    let provider_budget = &report.provider_work_budget;
    let source_evidence = serde_json::to_value(&report.source_evidence)?;
    let paraphrase_case_count = report
        .cases
        .iter()
        .filter(|case| case.intent_kind == SemanticIntentKind::Paraphrase)
        .count();
    let paraphrase_openai_top1 = report
        .cases
        .iter()
        .filter(|case| {
            case.intent_kind == SemanticIntentKind::Paraphrase && case.openai.top1_is_on_topic
        })
        .count();
    let paraphrase_hash_top1 = report
        .cases
        .iter()
        .filter(|case| {
            case.intent_kind == SemanticIntentKind::Paraphrase && case.hash.top1_is_on_topic
        })
        .count();
    // Rank-based aggregate (the discriminative one): paraphrase cases where openai
    // ranks the on-topic file strictly higher than hash, or retrieves it when hash
    // dropped it. recall@20 is NOT used here — it saturates on a small corpus.
    let paraphrase_openai_ranks_higher = report
        .cases
        .iter()
        .filter(|case| {
            case.intent_kind == SemanticIntentKind::Paraphrase
                && case.openai_ranks_on_topic_higher()
        })
        .count();
    Ok(json!({
        "status": "captured",
        "source_evidence": source_evidence,
        "resource_scope": "peak RSS of pinned Legacy daemon processes",
        "case_count": report.cases.len(),
        "paraphrase_case_count": paraphrase_case_count,
        "paraphrase_hash_top1": paraphrase_hash_top1,
        "paraphrase_openai_top1": paraphrase_openai_top1,
        "paraphrase_openai_ranks_on_topic_higher": paraphrase_openai_ranks_higher,
        "headline_metric": "on_topic_rank",
        "metrics_note": "on_topic_rank is the headline (finest) signal; mrr_at_10/ndcg_at_10 track it. recall@20 is a COARSE top-20-membership signal: vacuous on a <=20-doc corpus but meaningful on a larger one, where it catches files hash drops out of the top 20 entirely",
        // The registry captures the summary as the replay envelope. Retain
        // the same bounded observations and rankings that the sidecars expose.
        "provider_work_budget": provider_work_budget_json(provider_budget),
        "provider_stats": openai_provider_stats_json(&report.provider_stats),
        "case_results": openai_ab_cases_json(report),
        "runner_config": runner_config,
    }))
}

/// One artifact row per A/B case: the `OpenAI` side's produced page.
fn openai_ab_row(case: &SemanticAbCase) -> BenchRowV1 {
    BenchRowV1 {
        scenario_id: format!("relevance-openai-ab.{}", case.id),
        route_family: RouteFamily::Semantic,
        syntax: BenchSyntax::Native,
        result_shape: if case.openai.produced_order.is_empty() {
            ResultShape::Empty
        } else {
            ResultShape::Candidates
        },
        latency: None,
        qps: None,
        error_count: 0,
        timeout_count: 0,
        result_count: Some(saturating_u64(case.openai.produced_order.len())),
        typed_error_code: None,
        engine_touched: vec!["semantic".to_string()],
        early_stop_reason: None,
    }
}

fn openai_ab_config_digest(runner_config: &Value) -> String {
    config_digest(
        OPENAI_AB_DIMENSION,
        &[
            ("rank_k", RANK_K.to_string()),
            ("recall_k", RECALL_K.to_string()),
            ("cases", SEMANTIC_JUDGED_QUERIES.len().to_string()),
            ("runner_config", runner_config.to_string()),
        ],
    )
}

/// The A/B capture artifact: the semantic fixture's bytes, the `OpenAI`
/// profile's model revision, and one row per case.
pub fn openai_ab_artifact(
    report: &OpenAiSemanticAbReport,
    git_head: GitHeadV1,
    host: HostV1,
) -> AnyResult<BenchArtifactV1> {
    let runner_config = report.runner_config.clone();
    Ok(BenchArtifactV1 {
        dimension: OPENAI_AB_DIMENSION.to_string(),
        mode: BenchMode::Warm,
        concurrency: 1,
        provenance: BenchProvenanceV1 {
            git_head,
            corpus_digest: corpus_digest(
                OPENAI_AB_DIMENSION,
                &semantic_fixture_docs()
                    .iter()
                    .map(|(path, content)| ((*path).to_string(), (*content).to_string()))
                    .collect::<Vec<_>>(),
            ),
            config_digest: openai_ab_config_digest(&runner_config),
            model_revision: Some(report.model_revision.clone()),
        },
        host,
        resources: ResourceUsageV1 {
            peak_rss_bytes: report.peak_daemon_rss_bytes,
        },
        phases: PhaseDurationsV1::default(),
        disk_amplification: None,
        rows: report.cases.iter().map(openai_ab_row).collect(),
        detail: openai_ab_detail_json(report, runner_config)?,
    })
}

pub fn write_openai_ab_artifacts(
    report: &OpenAiSemanticAbReport,
    dir: &Path,
    git_head: GitHeadV1,
    host: HostV1,
) -> AnyResult<()> {
    use crate::artifact::write_json_pretty_noclobber;

    write_json_pretty_noclobber(
        &dir.join("summary.json"),
        &openai_ab_artifact(report, git_head, host)?,
    )?;
    write_json_pretty_noclobber(&dir.join("cases.json"), &openai_ab_cases_json(report))?;
    write_json_pretty_noclobber(
        &dir.join("provider-stats.json"),
        &openai_provider_stats_json(&report.provider_stats),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    //! Adversarial gate tests — prove the rail can go RED.
    //!
    //! A relevance gate that cannot fail is worthless. These feed deliberately
    //! wrong orderings into the pure scoring layer (no daemon needed) and assert
    //! the specific blocking failure fires. The seeded-corpus end-to-end pass is
    //! covered separately by the `relevance_matrix` binary.
    use super::*;
    use crate::relevance::corpus::OrderingInvariants;
    use quanta_index_embed::EmbeddingCacheRetentionPolicy;
    use quanta_index_searchd::app::config::OpenAiEmbedderTuning;

    fn fixture_query() -> JudgedQuery {
        JudgedQuery {
            id: "test.lexical",
            route: RelevanceRoute::Lexical,
            intent: "test intent",
            query: "needle",
            syntax: BenchSyntax::Native,
            judgments: &[("a", 3), ("b", 2), ("c", 1), ("neg", 0)],
            ordering: OrderingInvariants {
                top1: Some("a"),
                top_k_contains: &["a", "b", "c"],
                forbidden_within: &[("neg", 3)],
            },
        }
    }

    fn order(ids: &[&str]) -> Vec<String> {
        ids.iter().map(|s| (*s).to_string()).collect()
    }

    fn repo_paths(entries: &[(&str, &str)]) -> Vec<(String, String)> {
        entries
            .iter()
            .map(|(repo_id, path)| ((*repo_id).to_string(), (*path).to_string()))
            .collect()
    }

    fn ab_fixture_head(incarnation: u8) -> AnyResult<SearchCorpusActiveHeadV1> {
        use quanta_index_contract::{
            ACTIVATION_ROOT_INCARNATION_BYTES_V1, GenerationSnapshot, ManifestGeneration, RepoId,
            RevisionId, SearchCorpusActivationTokenV1, SearchCorpusGenerationIdentityV1,
            SearchPlaneTrackKind, SemanticContentRootsV1,
        };
        let lexical = GenerationSnapshot {
            repo_id: RepoId::new("repo-e2e")?,
            revision_id: RevisionId::new("rev-e2e")?,
            track: SearchPlaneTrackKind::Lexical,
            manifest_generation: ManifestGeneration::new(1),
            manifest_digest: "lex-seal:1".to_owned(),
        };
        let row_digest = format!("{incarnation:x}").repeat(64);
        Ok(SearchCorpusActiveHeadV1 {
            generation: SearchCorpusGenerationIdentityV1 {
                semantic: GenerationSnapshot {
                    track: SearchPlaneTrackKind::Semantic,
                    ..lexical.clone()
                },
                lexical,
                semantic_content: SemanticContentRootsV1 {
                    row_root_digest: format!("sha256:{row_digest}"),
                    membership_root_digest: format!("sha256:{}", "c".repeat(64)),
                },
            },
            activation_token: SearchCorpusActivationTokenV1::new(
                [incarnation; ACTIVATION_ROOT_INCARNATION_BYTES_V1],
                std::num::NonZeroU64::MIN,
            )?,
        })
    }

    #[test]
    fn openai_ab_approval_digest_matches_capture_contract() {
        let config = format!("sha256:{}", "1".repeat(64));
        let source = format!("sha256:{}", "2".repeat(64));
        let judgments = format!("sha256:{}", "3".repeat(64));
        assert_eq!(
            openai_ab_input_digest(&config, &source, &judgments),
            "sha256:c4e075295756e5653886b25e2ef802fd9628cd4c0a878bf08f80776fdcdd1d82"
        );
    }

    #[test]
    fn openai_ab_source_batch_is_deterministic() -> AnyResult<()> {
        let first = process::OpenAiAbFixture::prepare()?;
        let second = process::OpenAiAbFixture::prepare()?;
        assert_eq!(first.source_batch_digest(), second.source_batch_digest());
        Ok(())
    }

    #[test]
    fn openai_ab_summary_preserves_request_samples_and_rankings() -> AnyResult<()> {
        let query = SEMANTIC_JUDGED_QUERIES
            .iter()
            .find(|query| query.intent_kind == SemanticIntentKind::Paraphrase)
            .ok_or_else(|| anyhow::anyhow!("semantic judgment fixture lacks a paraphrase"))?;
        let profile = SemanticEmbedderProfile::OpenAi {
            model: "fixture-model".to_string(),
            model_revision: "fixture-revision".to_string(),
            dimension: 1536,
            api_key: "sentinel-secret-key".to_string(),
            tuning: OpenAiEmbedderTuning::default(),
        };
        let provider_work_budget = ProviderWorkBudgetConfig {
            total_cost_micros_ceiling: 37,
            openai_max_request_cost_micros: std::num::NonZeroU64::new(1),
            ..ProviderWorkBudgetConfig::default()
        };
        let grant_digest = "sha256:grant-fixture";
        let report = OpenAiSemanticAbReport {
            runner_config: openai_ab_runner_config_json(
                &SemanticEmbedderProfile::hash_dev(),
                &profile,
                &provider_work_budget,
                grant_digest,
            )?,
            model_revision: model_revision_of(&profile)
                .ok_or_else(|| anyhow::anyhow!("fixture OpenAI profile lacks model revision"))?,
            provider_work_budget,
            source_evidence: OpenAiAbSourceEvidence {
                legacy_daemon_sha256: "7".repeat(64),
                source_batch_digest: format!("sha256:{}", "8".repeat(64)),
                hash: ab_fixture_head(1)?,
                openai: ab_fixture_head(2)?,
                openai_publication_http_requests: 1,
                openai_publication_stats: json!({"http_request_count": 1}),
            },
            peak_daemon_rss_bytes: 4096,
            cases: vec![SemanticAbCase {
                id: query.id,
                intent_kind: query.intent_kind,
                intent: query.intent,
                query: query.query,
                on_topic_path: query.on_topic_path,
                hard_negative_path: query.hard_negative_path,
                hash: score_semantic_case(
                    query,
                    order(&[query.hard_negative_path, query.on_topic_path]),
                )?,
                openai: score_semantic_case(query, order(&[query.on_topic_path]))?,
            }],
            provider_stats: OpenAiEmbedStatsSnapshot {
                total_texts_observed: 260,
                cache_hits: 0,
                http_request_count: 257,
                distinct_miss_texts: 260,
                retry_count: 0,
                retryable_status_count: 0,
                transport_error_count: 0,
                max_request_texts: 4,
                max_estimated_tokens: 90,
                request_samples_dropped: 1,
                request_samples: vec![
                    quanta_index_embed::OpenAiRequestSample {
                        texts_submitted: 1,
                        estimated_tokens: 30,
                    };
                    256
                ],
            },
        };
        let runner_config = report.runner_config.clone();
        let detail = openai_ab_detail_json(&report, runner_config.clone())?;
        assert_eq!(detail["runner_config"], runner_config);
        assert!(!detail.to_string().contains("sentinel-secret-key"));
        assert_eq!(
            detail["source_evidence"]["legacy_daemon_sha256"],
            json!("7".repeat(64))
        );
        assert_eq!(
            detail["source_evidence"]["source_batch_digest"],
            json!(format!("sha256:{}", "8".repeat(64)))
        );
        assert_eq!(
            detail["source_evidence"]["hash"],
            serde_json::to_value(ab_fixture_head(1)?)?
        );
        assert_eq!(
            detail["source_evidence"]["openai"],
            serde_json::to_value(ab_fixture_head(2)?)?
        );
        assert_eq!(
            detail["source_evidence"]["openai_publication_http_requests"],
            json!(1)
        );
        assert_eq!(
            detail["runner_config"]["provider_egress_grant_digest"],
            json!("sha256:grant-fixture")
        );
        assert_eq!(
            detail["provider_work_budget"]["total_cost_micros_ceiling"],
            json!(37)
        );
        let stats = detail.get("provider_stats");
        assert_eq!(
            stats.and_then(|stats| stats.get("request_samples")),
            Some(&json!(vec![
                json!({"texts_submitted": 1, "estimated_tokens": 30});
                256
            ])),
        );
        assert_eq!(
            stats.and_then(|stats| stats.get("max_estimated_tokens")),
            Some(&json!(90)),
        );
        assert_eq!(
            stats,
            Some(&openai_provider_stats_json(&report.provider_stats))
        );
        let case_results = detail.get("case_results");
        assert_eq!(
            case_results.and_then(|cases| cases.pointer("/cases/0/delta/on_topic_rank_hash")),
            Some(&json!(2)),
        );
        assert_eq!(
            detail.get("paraphrase_openai_ranks_on_topic_higher"),
            Some(&json!(1))
        );
        assert_eq!(case_results, Some(&openai_ab_cases_json(&report)));

        let digest_for = |report: &OpenAiSemanticAbReport| -> AnyResult<String> {
            Ok(openai_ab_artifact(
                report,
                GitHeadV1::parse(&"0".repeat(40))?,
                HostV1 {
                    os: "fixture".to_string(),
                    arch: "fixture".to_string(),
                    cpu_count: 1,
                    mem_bytes: 1,
                    hostname_hash: "sha256:fixture".to_string(),
                },
            )?
            .provenance
            .config_digest)
        };
        let original = digest_for(&report)?;
        let artifact = openai_ab_artifact(
            &report,
            GitHeadV1::parse(&"0".repeat(40))?,
            HostV1 {
                os: "fixture".to_string(),
                arch: "fixture".to_string(),
                cpu_count: 1,
                mem_bytes: 1,
                hostname_hash: "sha256:fixture".to_string(),
            },
        )?;
        assert_eq!(artifact.resources.peak_rss_bytes, 4096);
        assert_eq!(artifact.detail["runner_config"], report.runner_config);
        assert_eq!(
            artifact.provenance.model_revision.as_deref(),
            Some("fixture-model@fixture-revision:d1536")
        );
        let mut different_key = profile.clone();
        if let SemanticEmbedderProfile::OpenAi { api_key, .. } = &mut different_key {
            *api_key = "different-sentinel-secret-key".to_string();
        }
        assert_eq!(
            openai_ab_runner_config_json(
                &SemanticEmbedderProfile::hash_dev(),
                &different_key,
                &report.provider_work_budget,
                grant_digest,
            )?,
            runner_config,
        );

        let tuning_changes: [(&str, fn(&mut OpenAiEmbedderTuning) -> AnyResult<()>); 13] = [
            ("max_batch", |tuning| {
                tuning.max_batch += 1;
                Ok(())
            }),
            ("max_estimated_tokens_per_request", |tuning| {
                tuning.max_estimated_tokens_per_request += 1;
                Ok(())
            }),
            ("max_retries", |tuning| {
                tuning.max_retries += 1;
                Ok(())
            }),
            ("timeout", |tuning| {
                tuning.timeout += std::time::Duration::from_secs(1);
                Ok(())
            }),
            ("timeout_subseconds", |tuning| {
                tuning.timeout += std::time::Duration::from_nanos(1);
                Ok(())
            }),
            ("cache_enabled", |tuning| {
                tuning.cache_enabled = !tuning.cache_enabled;
                Ok(())
            }),
            ("concurrency", |tuning| {
                tuning.concurrency += 1;
                Ok(())
            }),
            ("cache_max_entries", |tuning| {
                let policy = tuning.cache_retention;
                tuning.cache_retention = EmbeddingCacheRetentionPolicy::new(
                    policy.max_entries() + 1,
                    policy.max_resident_bytes(),
                    policy.max_namespaces(),
                    policy.max_entry_age(),
                    policy.max_total_bytes(),
                )?;
                Ok(())
            }),
            ("cache_max_resident_bytes", |tuning| {
                let policy = tuning.cache_retention;
                tuning.cache_retention = EmbeddingCacheRetentionPolicy::new(
                    policy.max_entries(),
                    policy.max_resident_bytes() + 1,
                    policy.max_namespaces(),
                    policy.max_entry_age(),
                    policy.max_total_bytes(),
                )?;
                Ok(())
            }),
            ("cache_max_namespaces", |tuning| {
                let policy = tuning.cache_retention;
                tuning.cache_retention = EmbeddingCacheRetentionPolicy::new(
                    policy.max_entries(),
                    policy.max_resident_bytes(),
                    policy.max_namespaces() + 1,
                    policy.max_entry_age(),
                    policy.max_total_bytes(),
                )?;
                Ok(())
            }),
            ("cache_max_entry_age", |tuning| {
                let policy = tuning.cache_retention;
                tuning.cache_retention = EmbeddingCacheRetentionPolicy::new(
                    policy.max_entries(),
                    policy.max_resident_bytes(),
                    policy.max_namespaces(),
                    policy.max_entry_age() + std::time::Duration::from_secs(1),
                    policy.max_total_bytes(),
                )?;
                Ok(())
            }),
            ("cache_max_total_bytes", |tuning| {
                let policy = tuning.cache_retention;
                tuning.cache_retention = EmbeddingCacheRetentionPolicy::new(
                    policy.max_entries(),
                    policy.max_resident_bytes(),
                    policy.max_namespaces(),
                    policy.max_entry_age(),
                    policy.max_total_bytes() + 1,
                )?;
                Ok(())
            }),
            ("cache_age_subseconds", |tuning| {
                let policy = tuning.cache_retention;
                tuning.cache_retention = EmbeddingCacheRetentionPolicy::new(
                    policy.max_entries(),
                    policy.max_resident_bytes(),
                    policy.max_namespaces(),
                    policy.max_entry_age() + std::time::Duration::from_nanos(1),
                    policy.max_total_bytes(),
                )?;
                Ok(())
            }),
        ];
        for (name, change) in tuning_changes {
            let mut changed = profile.clone();
            if let SemanticEmbedderProfile::OpenAi { tuning, .. } = &mut changed {
                change(tuning)?;
            }
            let mut changed_report = report.clone();
            changed_report.runner_config = openai_ab_runner_config_json(
                &SemanticEmbedderProfile::hash_dev(),
                &changed,
                &report.provider_work_budget,
                grant_digest,
            )?;
            assert_ne!(digest_for(&changed_report)?, original, "{name}");
        }
        let mut changed_grant = report.clone();
        changed_grant.runner_config = openai_ab_runner_config_json(
            &SemanticEmbedderProfile::hash_dev(),
            &profile,
            &report.provider_work_budget,
            "sha256:other-grant-fixture",
        )?;
        assert_ne!(digest_for(&changed_grant)?, original);
        let mut changed_budget = report.clone();
        changed_budget.provider_work_budget.retry_attempts_cap += 1;
        changed_budget.runner_config = openai_ab_runner_config_json(
            &SemanticEmbedderProfile::hash_dev(),
            &profile,
            &changed_budget.provider_work_budget,
            grant_digest,
        )?;
        assert_ne!(digest_for(&changed_budget)?, original);
        Ok(())
    }

    #[test]
    fn model_quality_gate_requires_nonempty_discriminative_ranks() {
        let perfect = SemanticCaseMetrics {
            produced_order: vec!["on-topic.rs".to_string()],
            mrr_at_10: 1.0,
            ndcg_at_10: 1.0,
            recall_at_20: 1.0,
            top1_is_on_topic: true,
            hard_negative_rank: None,
            on_topic_rank: Some(1),
        };
        assert!(!summarize_semantic_quality(&[]).passed);
        assert!(
            !summarize_semantic_quality(&[perfect.clone(), perfect.clone()]).passed,
            "a partial paraphrase run must not pass on high scores"
        );
        assert!(!summarize_semantic_quality(&vec![perfect.clone(); 11]).passed);
        assert!(summarize_semantic_quality(&vec![perfect.clone(); 12]).passed);
        assert!(!summarize_semantic_quality(&vec![perfect.clone(); 13]).passed);
        let missed = SemanticCaseMetrics {
            produced_order: vec!["hard-negative.rs".to_string()],
            mrr_at_10: 0.0,
            ndcg_at_10: 0.0,
            recall_at_20: 0.0,
            top1_is_on_topic: false,
            hard_negative_rank: Some(1),
            on_topic_rank: None,
        };
        assert!(!summarize_semantic_quality(&[perfect, missed]).passed);
    }

    #[test]
    fn lexical_symbol_tokens_accept_definition_or_call_first_but_not_a_negative() {
        let query = JUDGED_QUERIES.first().expect("judged query fixture exists");
        for ranking in [
            [
                "src/config/parser.rs",
                "src/config/loader.rs",
                "src/config/mod.rs",
            ],
            [
                "src/config/loader.rs",
                "src/config/mod.rs",
                "src/config/parser.rs",
            ],
        ] {
            let scored = score_query(query, order(&ranking)).expect("valid lexical judgment");
            assert!(scored.passed(), "{ranking:?}: {:?}", scored.failures);
        }
        let scored = score_query(
            query,
            order(&[
                "src/net/client.rs",
                "src/config/loader.rs",
                "src/config/mod.rs",
                "src/config/parser.rs",
            ]),
        )
        .expect("valid lexical judgment");
        assert!(
            !scored.passed(),
            "token-overlap negative must fail the gate"
        );
    }

    // --- pure same-path-collapse oracle (no daemon) ---------------------

    #[test]
    fn collapse_same_repo_path_keeps_first_occurrence_only() {
        // A file hit at ranks 1 and 3 must occupy exactly one (earliest) rank.
        let collapsed = collapse_same_repo_path(repo_paths(&[
            ("repo-a", "a.rs"),
            ("repo-a", "b.rs"),
            ("repo-a", "a.rs"),
            ("repo-a", "c.rs"),
            ("repo-a", "b.rs"),
        ]));
        assert_eq!(
            collapsed,
            order(&["a.rs", "b.rs", "c.rs"]),
            "repeated paths must collapse to their earliest occurrence, preserving order"
        );
    }

    #[test]
    fn collapse_same_repo_path_is_identity_when_already_unique() {
        let input = repo_paths(&[("repo-a", "x.rs"), ("repo-a", "y.rs"), ("repo-a", "z.rs")]);
        assert_eq!(
            collapse_same_repo_path(input),
            order(&["x.rs", "y.rs", "z.rs"]),
            "an already-unique ordering must be returned unchanged"
        );
    }

    #[test]
    fn collapse_same_repo_path_is_deterministic_across_repeated_calls() {
        let input = repo_paths(&[
            ("repo-a", "p.rs"),
            ("repo-a", "q.rs"),
            ("repo-a", "p.rs"),
            ("repo-a", "r.rs"),
            ("repo-a", "q.rs"),
            ("repo-a", "p.rs"),
        ]);
        let first = collapse_same_repo_path(input.clone());
        let second = collapse_same_repo_path(input);
        assert_eq!(
            first, second,
            "collapse is a pure function: same input must yield same output"
        );
    }

    #[test]
    fn collapse_same_repo_path_keeps_same_path_from_different_repos_distinct() {
        let collapsed = collapse_same_repo_path(repo_paths(&[
            ("repo-a", "shared.rs"),
            ("repo-b", "shared.rs"),
            ("repo-a", "shared.rs"),
        ]));
        assert_eq!(
            collapsed,
            order(&["shared.rs", "shared.rs"]),
            "same path text in different repos must NOT collapse into one rank"
        );
    }

    #[test]
    fn correct_order_passes_all_gates() {
        let score = score_query(&fixture_query(), order(&["a", "b", "c", "neg"])).unwrap();
        assert!(score.passed(), "failures: {:?}", score.failures);
    }

    #[test]
    fn demoted_top1_trips_invariant_and_mrr() {
        // best answer `a` demoted below `b`.
        let score = score_query(&fixture_query(), order(&["b", "a", "c", "neg"])).unwrap();
        assert!(!score.passed());
        assert!(
            score.failures.iter().any(|f| f.contains("top1 invariant")),
            "{:?}",
            score.failures
        );
    }

    #[test]
    fn hard_negative_in_head_trips_forbidden_within() {
        // `neg` (token-overlap distractor) reaches rank 2.
        let score = score_query(&fixture_query(), order(&["a", "neg", "b", "c"])).unwrap();
        assert!(!score.passed());
        assert!(
            score
                .failures
                .iter()
                .any(|f| f.contains("forbidden_within")),
            "{:?}",
            score.failures
        );
    }

    #[test]
    fn missing_relevant_doc_trips_containment_and_recall() {
        // `c` dropped entirely from the ranking.
        let score = score_query(&fixture_query(), order(&["a", "b", "neg"])).unwrap();
        assert!(!score.passed());
        assert!(
            score.failures.iter().any(|f| f.contains("top_k_contains")),
            "{:?}",
            score.failures
        );
        assert!(
            score.failures.iter().any(|f| f.contains("Recall@20")),
            "{:?}",
            score.failures
        );
    }

    #[test]
    fn empty_ranking_fails_closed() {
        let score = score_query(&fixture_query(), Vec::new()).unwrap();
        assert!(!score.passed());
        // every relevant doc missing -> recall 0, top1 empty.
        assert!(score.failures.iter().any(|f| f.contains("top1 invariant")));
    }

    #[test]
    fn judgment_artifact_refuses_a_missing_produced_ranking() {
        let report = RelevanceReport {
            queries: Vec::new(),
            routes: Vec::new(),
            overlap: Vec::new(),
            passed: false,
            model_revision: None,
            semantic_quality: None,
            peak_daemon_rss_bytes: 1,
            legacy_daemon_sha256: "0".repeat(64),
        };
        let error = judgments_json(&report).expect_err("missing ranked query must not serialize");
        let first = JUDGED_QUERIES
            .first()
            .expect("the judged corpus is nonempty");
        assert!(error.to_string().contains(first.id));
    }

    #[test]
    #[expect(
        clippy::indexing_slicing,
        reason = "test indexes the overlap JSON shape this module constructs; an out-of-range index is a legitimate test failure"
    )]
    fn sourcegraph_overlap_artifact_is_unprovisioned_with_captured_quanta_half() {
        // J7Q-01B contract shape: the quanta half is captured + persisted, the
        // Sourcegraph half is unprovisioned, and the floor is explicitly NOT met
        // — no row can read as a competitive pass.
        let report = RelevanceReport {
            queries: Vec::new(),
            routes: Vec::new(),
            overlap: vec![OverlapCapture {
                bucket: "keyword",
                query_family: "literal keyword match",
                query: "parse_config",
                syntax: BenchSyntax::Native,
                sourcegraph_surface: "sourcegraph lexical (literal pattern)",
                quanta_ordering: order(&["src/config/parser.rs", "src/config/loader.rs"]),
                quanta_capture_error: None,
            }],
            passed: true,
            model_revision: None,
            semantic_quality: None,
            peak_daemon_rss_bytes: 1,
            legacy_daemon_sha256: "0".repeat(64),
        };
        let value = sourcegraph_overlap_json(&report, "2026-06-09");
        assert_eq!(value["status"], "unprovisioned");
        assert_eq!(value["external_floor_met"], false);
        assert_eq!(value["capture_date"], "2026-06-09");
        assert_eq!(value["overlap_buckets"].as_array().map(Vec::len), Some(6));
        let row = &value["comparisons"][0];
        assert_eq!(row["bucket"], "keyword");
        assert_eq!(row["query"], "parse_config");
        assert_eq!(row["syntax"], "native");
        assert_eq!(row["capture_date"], "2026-06-09");
        // The quanta half is real; the Sourcegraph half + verdict stay unprovisioned.
        assert_eq!(row["quanta_index_ordering"][0], "src/config/parser.rs");
        assert_eq!(row["sourcegraph_ordering"], "unprovisioned");
        assert_eq!(row["verdict"], "unprovisioned");
    }
}
