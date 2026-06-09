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
    JUDGED_QUERIES, JudgedQuery, LEXICAL_RELEVANCE_CORPUS, RELEVANCE_REPO, RelevanceRoute, TOP_K,
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

/// The full relevance report.
#[derive(Clone, Debug)]
pub struct RelevanceReport {
    pub queries: Vec<QueryScore>,
    pub routes: Vec<RouteSummary>,
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

/// Run the full relevance rail against a freshly seeded runtime.
pub fn run_relevance_report() -> AnyResult<RelevanceReport> {
    let mut rt = prepare_relevance_runtime()?;
    let mut queries = Vec::with_capacity(JUDGED_QUERIES.len());
    for query in JUDGED_QUERIES {
        let order = produced_order(&mut rt, query)?;
        queries.push(score_query(query, order)?);
    }
    let routes = aggregate_routes(&queries);
    let passed = queries.iter().all(QueryScore::passed) && routes.iter().all(|r| r.passed);
    Ok(RelevanceReport {
        queries,
        routes,
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

/// The Sourcegraph lexical overlap stub.
///
/// Emitted as `unprovisioned` (J7Q-01B side lane). Recording it as a real,
/// non-passing artifact keeps the external floor honest instead of letting its
/// absence read as success.
fn sourcegraph_overlap_json() -> Value {
    json!({
        "schema_version": 1,
        "status": "unprovisioned",
        "owner_ticket": "J7Q-01B",
        "external_floor_met": false,
        "note": "local Sourcegraph provisioning + ordering capture not yet implemented",
        "overlap_buckets": ["keyword", "phrase", "regex", "path-constrained content", "repo metadata", "symbol name"],
        "comparisons": [],
    })
}

/// Write the three canonical relevance artifacts under `dir`.
pub fn write_artifacts(report: &RelevanceReport, dir: &Path, git_rev: &str) -> AnyResult<()> {
    crate::artifact::write_json_pretty(&dir.join("summary.json"), &summary_json(report, git_rev))?;
    crate::artifact::write_json_pretty(
        &dir.join("query_judgments.json"),
        &judgments_json(report),
    )?;
    crate::artifact::write_json_pretty(
        &dir.join("sourcegraph-overlap.json"),
        &sourcegraph_overlap_json(),
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
}
