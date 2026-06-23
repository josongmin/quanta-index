//! Relevance rail run engine: seed → query → score → gate → artifact.
//!
//! Boots one sealed generation over the judged fixture, runs each
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
use quanta_index_contract::TextQuerySyntax;
use serde_json::{Value, json};

use crate::artifact::BenchSyntax;
use crate::harness::E2eRuntime;
use crate::relevance::corpus::{
    JUDGED_QUERIES, JudgedQuery, LEXICAL_RELEVANCE_CORPUS, RELEVANCE_REPO, RelevanceRoute,
    SEMANTIC_GATED_QUERIES, SEMANTIC_RELEVANCE_CORPUS, SEMANTIC_RELEVANCE_REPO,
    SOURCEGRAPH_OVERLAP_BUCKETS, SourcegraphOverlapBucket, TOP_K,
};
#[cfg(test)]
use crate::relevance::corpus::{SEMANTIC_JUDGED_QUERIES, SemanticIntentKind};
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
}

fn to_text_syntax(syntax: BenchSyntax) -> TextQuerySyntax {
    match syntax {
        BenchSyntax::Native => TextQuerySyntax::Native,
        BenchSyntax::Sourcegraph => TextQuerySyntax::Sourcegraph,
    }
}

/// Boot one runtime, seed the judged fixture, seal + activate.
pub fn prepare_relevance_runtime() -> AnyResult<E2eRuntime> {
    let mut rt = E2eRuntime::boot()?;
    for (path, content) in LEXICAL_RELEVANCE_CORPUS {
        rt.ingest_text(RELEVANCE_REPO, path, content)?;
    }
    let _generation = rt.seal()?;
    rt.activate_last_sealed_generation()?;
    Ok(rt)
}

/// Boot a HASH-embedder runtime over the SEMANTIC fixture and activate the
/// Lexical + Semantic tracks so `query_semantic` has a live vector index.
///
/// Seeds ONLY the semantic corpus (the lexical corpus is intentionally absent so
/// the semantic route's produced ids stay within the semantic fixture, never
/// blurred by lexical-corpus paths). `auth/token_refresh.rs` is ingested as TWO
/// chunks so the same-path-collapse rule has repeated hits to collapse. Asserting
/// the `Hash` profile keeps the CI semantic gate deterministic and
/// neural-quality-free (RFC §5 P1-3); the OpenAI A/B is a separate local lane.
pub fn prepare_semantic_relevance_runtime() -> AnyResult<E2eRuntime> {
    use crate::harness::E2eTextChunkSpec;
    use quanta_index_contract::SearchPlaneTrackKind;

    let mut rt = E2eRuntime::boot()?;
    if !matches!(
        rt.embedder_profile(),
        quanta_index_searchd::app::SemanticEmbedderProfile::Hash { .. }
    ) {
        return Err(anyhow::anyhow!(
            "the semantic CI gate MUST run on the deterministic Hash embedder; got {:?}",
            rt.embedder_profile()
        ));
    }
    for (path, content) in SEMANTIC_RELEVANCE_CORPUS {
        if *path == "auth/token_refresh.rs" {
            // Two chunks, same path -> exercises same-path collapse on the rail.
            let half = content.len() / 2;
            let (head, tail) = content.split_at(half);
            let _ids = rt.ingest_text_chunks(
                SEMANTIC_RELEVANCE_REPO,
                path,
                &[
                    E2eTextChunkSpec {
                        content: head,
                        start_line: 1,
                        end_line: 2,
                        source_repo_id: None,
                    },
                    E2eTextChunkSpec {
                        content: tail,
                        start_line: 3,
                        end_line: 4,
                        source_repo_id: None,
                    },
                ],
            )?;
        } else {
            rt.ingest_text(SEMANTIC_RELEVANCE_REPO, path, content)?;
        }
    }
    let _generation = rt.seal_lexical_generation_for_tracks(&[
        SearchPlaneTrackKind::Lexical,
        SearchPlaneTrackKind::Semantic,
    ])?;
    rt.activate_last_sealed_generation_with_tracks(&[
        SearchPlaneTrackKind::Lexical,
        SearchPlaneTrackKind::Semantic,
    ])?;
    Ok(rt)
}

/// Collapse repeated same-path hits to the earliest occurrence, preserving the
/// produced order of first occurrences.
///
/// The semantic route is scored at file granularity, but the engine returns one
/// candidate per CHUNK, so a single file can appear several times. Collapsing to
/// the first occurrence (RFC §5 P1-2 / §8) keeps one file from occupying
/// multiple head ranks and from inflating or obscuring file-level metrics. This
/// is deterministic: given the same produced order it returns the same collapsed
/// order.
fn collapse_same_path(paths: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::BTreeSet::new();
    let mut collapsed = Vec::with_capacity(paths.len());
    for path in paths {
        if seen.insert(path.clone()) {
            collapsed.push(path);
        }
    }
    collapsed
}

/// Run one judged query and return its produced doc-id ordering (top-first).
///
/// Fails closed: a typed error on a relevance query is a rail failure, never an
/// empty-ranking pass.
fn produced_order(rt: &mut E2eRuntime, query: &JudgedQuery) -> AnyResult<Vec<String>> {
    let syntax = to_text_syntax(query.syntax);
    match query.route {
        RelevanceRoute::Lexical => {
            let result = rt.query_text(syntax, query.query, TOP_K);
            if let Some(error) = result.typed_error {
                return Err(anyhow::anyhow!(
                    "relevance query `{}` returned typed error {}: {}",
                    query.id,
                    error.code,
                    error.message
                ));
            }
            Ok(result
                .candidates
                .iter()
                .map(|candidate| candidate.repo_relative_path.as_str().to_string())
                .collect())
        }
        RelevanceRoute::Semantic => {
            // Pure vector ranking: no lexical scope, so the semantic searcher
            // ranks the whole index by embedding similarity (NOT a lexical-scoped
            // re-rank, which would blur semantic-route truth — RFC §5 P1-2).
            let result = rt.query_semantic(query.query, TOP_K, None);
            if let Some(error) = result.typed_error {
                return Err(anyhow::anyhow!(
                    "relevance query `{}` returned typed error {}: {}",
                    query.id,
                    error.code,
                    error.message
                ));
            }
            // Score by repo-relative path (NOT raw chunk id), then collapse
            // repeated same-path hits to the earliest occurrence.
            let paths = result
                .candidates
                .iter()
                .map(|candidate| candidate.repo_relative_path.as_str().to_string())
                .collect();
            Ok(collapse_same_path(paths))
        }
    }
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

/// Capture the live quanta-index ordering for one Sourcegraph overlap bucket.
///
/// Fail-soft (NOT fail-closed-as-abort): a typed query error is recorded in
/// `quanta_capture_error` and the ordering is left empty, because the overlap is
/// an external side lane whose verdict is `unprovisioned` regardless — a capture
/// error must not turn the whole relevance rail red over an absent dependency.
/// The error is surfaced verbatim in the artifact, never swallowed.
fn capture_overlap_bucket(
    rt: &mut E2eRuntime,
    bucket: &SourcegraphOverlapBucket,
) -> OverlapCapture {
    let result = rt.query_text(to_text_syntax(bucket.syntax), bucket.query, TOP_K);
    let (quanta_ordering, quanta_capture_error) = match result.typed_error {
        Some(error) => (
            Vec::new(),
            Some(format!("{}: {}", error.code, error.message)),
        ),
        None => (
            result
                .candidates
                .iter()
                .map(|candidate| candidate.repo_relative_path.as_str().to_string())
                .collect(),
            None,
        ),
    };
    OverlapCapture {
        bucket: bucket.bucket,
        query_family: bucket.query_family,
        query: bucket.query,
        syntax: bucket.syntax,
        sourcegraph_surface: bucket.sourcegraph_surface,
        quanta_ordering,
        quanta_capture_error,
    }
}

/// Run the full relevance rail against a freshly seeded runtime.
pub fn run_relevance_report() -> AnyResult<RelevanceReport> {
    let mut rt = prepare_relevance_runtime()?;
    let mut queries = Vec::with_capacity(JUDGED_QUERIES.len() + SEMANTIC_GATED_QUERIES.len());
    for query in JUDGED_QUERIES {
        let order = produced_order(&mut rt, query)?;
        queries.push(score_query(query, order)?);
    }
    // Semantic route, gated on what the deterministic Hash embedder genuinely
    // owns: exact-token recall (RFC §5 P1-3). Runs on a SEMANTIC-track runtime so
    // `query_semantic` has a live vector index; paraphrase queries are NOT gated
    // here (neural-only — covered as determinism-only unit tests). This is a real
    // rail gate + artifact row, not a test-only probe.
    let mut sem_rt = prepare_semantic_relevance_runtime()?;
    for query in SEMANTIC_GATED_QUERIES {
        let order = produced_order(&mut sem_rt, query)?;
        queries.push(score_query(query, order)?);
    }
    let routes = aggregate_routes(&queries);
    // J7Q-01B: capture the quanta-index half of every overlap bucket live. The
    // Sourcegraph half stays unprovisioned, so this never gates `passed`.
    let overlap = SOURCEGRAPH_OVERLAP_BUCKETS
        .iter()
        .map(|bucket| capture_overlap_bucket(&mut rt, bucket))
        .collect();
    let passed = queries.iter().all(QueryScore::passed) && routes.iter().all(|r| r.passed);
    Ok(RelevanceReport {
        queries,
        routes,
        overlap,
        passed,
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
fn judgments_json(report: &RelevanceReport) -> Value {
    let rows: Vec<Value> = JUDGED_QUERIES
        .iter()
        .chain(SEMANTIC_GATED_QUERIES.iter())
        .map(|q| {
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
                .unwrap_or_default();
            json!({
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
            })
        })
        .collect();
    json!({ "schema_version": 1, "queries": rows })
}

fn summary_json(report: &RelevanceReport, git_rev: &str) -> Value {
    json!({
        "schema_version": 1,
        "dimension": "relevance",
        "git_rev": git_rev,
        "passed": report.passed,
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
        "schema_version": 1,
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
    git_rev: &str,
    capture_date: &str,
) -> AnyResult<()> {
    crate::artifact::write_json_pretty(&dir.join("summary.json"), &summary_json(report, git_rev))?;
    crate::artifact::write_json_pretty(&dir.join("query_judgments.json"), &judgments_json(report))?;
    crate::artifact::write_json_pretty(
        &dir.join("sourcegraph-overlap.json"),
        &sourcegraph_overlap_json(report, capture_date),
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

    // --- pure same-path-collapse oracle (no daemon) ---------------------

    #[test]
    fn collapse_same_path_keeps_first_occurrence_only() {
        // A file hit at ranks 1 and 3 must occupy exactly one (earliest) rank.
        let collapsed = collapse_same_path(order(&["a.rs", "b.rs", "a.rs", "c.rs", "b.rs"]));
        assert_eq!(
            collapsed,
            order(&["a.rs", "b.rs", "c.rs"]),
            "repeated paths must collapse to their earliest occurrence, preserving order"
        );
    }

    #[test]
    fn collapse_same_path_is_identity_when_already_unique() {
        let input = order(&["x.rs", "y.rs", "z.rs"]);
        assert_eq!(
            collapse_same_path(input.clone()),
            input,
            "an already-unique ordering must be returned unchanged"
        );
    }

    #[test]
    fn collapse_same_path_is_deterministic_across_repeated_calls() {
        let input = order(&["p.rs", "q.rs", "p.rs", "r.rs", "q.rs", "p.rs"]);
        let first = collapse_same_path(input.clone());
        let second = collapse_same_path(input);
        assert_eq!(
            first, second,
            "collapse is a pure function: same input must yield same output"
        );
    }

    // --- daemon-driven semantic route (Hash embedder, RFC P1-1/P1-2/P1-3) -

    /// Build a `Semantic`-route judged query so the test drives the REAL
    /// production dispatch arm (`produced_order` -> `query_semantic(None)` ->
    /// path projection -> `collapse_same_path`), not a parallel re-implementation.
    fn semantic_route_query(id: &'static str, query: &'static str) -> JudgedQuery {
        JudgedQuery {
            id,
            route: RelevanceRoute::Semantic,
            intent: "semantic route mechanics probe",
            query,
            syntax: BenchSyntax::Native,
            judgments: &[],
            ordering: OrderingInvariants {
                top1: None,
                top_k_contains: &[],
                forbidden_within: &[],
            },
        }
    }

    /// Boot a HASH-embedder runtime, seed the semantic fixture, and activate the
    /// Lexical + Semantic tracks. Delegates to the shared production helper so the
    /// unit coverage and the gated `run_relevance_report` rail seed the SAME way
    /// (no test/prod divergence). `auth/token_refresh.rs` is ingested as TWO
    /// chunks so the same-path-collapse rule has repeated hits to collapse.
    fn prepare_semantic_runtime() -> AnyResult<E2eRuntime> {
        super::prepare_semantic_relevance_runtime()
    }

    #[test]
    fn semantic_route_projects_paths_and_collapses_same_file() {
        // RFC P1-2: the semantic route MUST score by repo_relative_path (never a
        // raw chunk id) AND collapse repeated same-path hits to one rank.
        let mut rt = prepare_semantic_runtime().expect("semantic runtime boots on hash");
        let q = semantic_route_query("sem.mechanics.path_collapse", "refresh auth token expires");
        let order = produced_order(&mut rt, &q).expect("semantic route returns an ordering");

        assert!(
            !order.is_empty(),
            "an exact-token semantic query must retrieve at least one path under hash"
        );
        // Path projection: every returned id is a corpus path, never an engine
        // chunk id (chunk ids are `e2e-<n>-<path>` shaped).
        let corpus_paths: std::collections::BTreeSet<&str> =
            SEMANTIC_RELEVANCE_CORPUS.iter().map(|(p, _)| *p).collect();
        for doc in &order {
            assert!(
                corpus_paths.contains(doc.as_str()),
                "semantic route returned `{doc}`, which is not a corpus path \
                 (raw chunk id leak / wrong projection)"
            );
        }
        // Same-path collapse: even though `auth/token_refresh.rs` was ingested as
        // two chunks, it appears at most once in the projected ordering.
        let refresh_hits = order
            .iter()
            .filter(|d| d.as_str() == "auth/token_refresh.rs")
            .count();
        assert!(
            refresh_hits <= 1,
            "auth/token_refresh.rs (2 chunks) must collapse to <=1 rank, saw {refresh_hits}: {order:?}"
        );
        // No path may appear twice after collapse.
        let mut unique = order.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(
            unique.len(),
            order.len(),
            "collapsed semantic ordering must contain no duplicate paths: {order:?}"
        );
    }

    #[test]
    fn semantic_route_is_deterministic_under_hash() {
        // RFC P1-3 honesty: assert determinism (same input => same ranked order),
        // NOT neural quality. Two independent runs over identical seeded input
        // must produce byte-identical orderings AND identical metric vectors.
        let mut rt_a = prepare_semantic_runtime().expect("run A boots");
        let q = semantic_route_query("sem.determinism", "refresh auth token expires");
        let order_a1 = produced_order(&mut rt_a, &q).expect("run A query 1");
        let order_a2 = produced_order(&mut rt_a, &q).expect("run A query 2");
        assert_eq!(
            order_a1, order_a2,
            "same query on the same generation must be stable within a run"
        );

        let mut rt_b = prepare_semantic_runtime().expect("run B boots");
        let order_b = produced_order(&mut rt_b, &q).expect("run B query");
        assert_eq!(
            order_a1, order_b,
            "identical seeded input must yield an identical hash ranking across runs"
        );

        // Metrics over the same ordering are likewise a pure function.
        let judged = vec![GradedDoc {
            doc_id: "auth/token_refresh.rs".to_string(),
            grade: 3,
        }];
        let grades = grade_index(&judged).expect("grade index");
        assert_eq!(
            recall_at_k(&order_a1, &grades, RECALL_K),
            recall_at_k(&order_b, &grades, RECALL_K),
            "recall must be identical across deterministic runs"
        );
    }

    #[test]
    fn semantic_exact_token_query_ranks_on_topic_file_first_under_hash() {
        // RFC P1-3: the semantic-quality claim safe for hash is exact-token rank.
        // Non-vacuity (R-TEST-19/20): mere retrieval (`any(== on_topic)`) is NOT
        // discriminative on a <20-doc fixture (every doc is always returned), so
        // require the on-topic file at RANK 1 — for a single-best-match exact-token
        // query the file with verbatim overlap deterministically outranks the
        // ~zero-overlap rest. Still do NOT assert nDCG/MRR tie-order.
        let mut rt = prepare_semantic_runtime().expect("semantic runtime boots");
        let mut exact_token_seen = 0_usize;
        for jq in SEMANTIC_JUDGED_QUERIES {
            if jq.intent_kind != SemanticIntentKind::ExactToken {
                continue;
            }
            exact_token_seen = exact_token_seen.saturating_add(1);
            let q = semantic_route_query(jq.id, jq.query);
            let order = produced_order(&mut rt, &q).expect("exact-token semantic query runs");
            assert_eq!(
                order.first().map(String::as_str),
                Some(jq.on_topic_path),
                "exact-token query `{}` must rank its on-topic file `{}` at rank 1 under hash; got {order:?}",
                jq.id,
                jq.on_topic_path
            );
        }
        assert!(
            exact_token_seen > 0,
            "the semantic fixture MUST contain at least one ExactToken judged query"
        );
    }

    #[test]
    fn semantic_paraphrase_query_runs_deterministically_without_quality_claim() {
        // RFC P1-3 honesty: the paraphrase layer is the discriminative subset a
        // NEURAL embedder separates. Under hash we assert ONLY that the route runs
        // and is deterministic — never that hash recalls the paraphrase target
        // (it is expected not to). This documents the limit instead of faking it.
        let mut rt = prepare_semantic_runtime().expect("semantic runtime boots");
        let mut paraphrase_seen = 0_usize;
        for jq in SEMANTIC_JUDGED_QUERIES {
            if jq.intent_kind != SemanticIntentKind::Paraphrase {
                continue;
            }
            paraphrase_seen = paraphrase_seen.saturating_add(1);
            let q = semantic_route_query(jq.id, jq.query);
            let first = produced_order(&mut rt, &q).expect("paraphrase query run 1");
            let second = produced_order(&mut rt, &q).expect("paraphrase query run 2");
            assert_eq!(
                first, second,
                "paraphrase query `{}` must be deterministic under hash (mechanics, not quality)",
                jq.id
            );
            // Every returned id is still a path, never a raw chunk id.
            let corpus_paths: std::collections::BTreeSet<&str> =
                SEMANTIC_RELEVANCE_CORPUS.iter().map(|(p, _)| *p).collect();
            for doc in &first {
                assert!(
                    corpus_paths.contains(doc.as_str()),
                    "paraphrase route returned non-path id `{doc}`"
                );
            }
        }
        assert!(
            paraphrase_seen > 0,
            "the semantic fixture MUST contain at least one Paraphrase judged query (local discriminative layer)"
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
