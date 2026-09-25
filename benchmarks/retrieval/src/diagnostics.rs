//! Record-bound, non-scoring SDK retrieval diagnostics.
//!
//! This artifact is deliberately separate from the frozen v3 runner record.
//! It exposes the returned candidate window and hybrid lane contributions,
//! not the unreturned lane universe or server-internal timing.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};

use crate::record::QueryPack;
use crate::sdk::QueryOutcome;
use crate::{BenchError, BenchResult};

pub fn diagnostic_value(
    record_sha256: &str,
    pack: &QueryPack,
    routes: &[&str],
    outcomes: &BTreeMap<(String, String), QueryOutcome>,
    top_k: u32,
) -> BenchResult<Value> {
    if record_sha256.len() != 64
        || !record_sha256
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(BenchError::Protocol(
            "diagnostic record digest must be lowercase sha256".to_string(),
        ));
    }
    if top_k == 0 || top_k != pack.contract_top_k {
        return Err(BenchError::Protocol(
            "diagnostic top_k differs from the query pack".to_string(),
        ));
    }
    let route_set: BTreeSet<&str> = routes.iter().copied().collect();
    if route_set.len() != routes.len()
        || route_set != pack.routes.iter().map(String::as_str).collect()
    {
        return Err(BenchError::Protocol(
            "diagnostic routes differ from the query pack".to_string(),
        ));
    }
    let expected: BTreeSet<(String, String)> = pack
        .tasks
        .iter()
        .flat_map(|task| {
            routes
                .iter()
                .map(move |route| (task.task_id.clone(), (*route).to_string()))
        })
        .collect();
    if expected.len() != pack.tasks.len() * routes.len()
        || outcomes.keys().cloned().collect::<BTreeSet<_>>() != expected
    {
        return Err(BenchError::Protocol(
            "diagnostic outcomes are missing, duplicated or unexpected".to_string(),
        ));
    }
    let mut rows = Vec::with_capacity(expected.len());
    for task in &pack.tasks {
        for route in routes {
            let outcome = outcomes
                .get(&(task.task_id.clone(), (*route).to_string()))
                .ok_or_else(|| {
                    BenchError::Protocol("diagnostic outcome disappeared".to_string())
                })?;
            let (status, error_code, candidates) = match outcome {
                QueryOutcome::Hits { hits, outcome, .. } => {
                    if hits.len() > top_k as usize {
                        return Err(BenchError::Protocol(format!(
                            "diagnostic hit count exceeds top_k for {}/{}",
                            task.task_id, route
                        )));
                    }
                    let status = if hits.is_empty() {
                        if !outcome.is_exhausted() {
                            return Err(BenchError::Protocol(format!(
                                "diagnostic empty non-exhausted result for {}/{}",
                                task.task_id, route
                            )));
                        }
                        "abstained"
                    } else if outcome.is_exhausted() {
                        "success"
                    } else {
                        "capped"
                    };
                    let mut candidates = Vec::with_capacity(hits.len());
                    for (position, hit) in hits.iter().enumerate() {
                        if !hit.score.is_finite() || hit.candidate_id.is_empty() {
                            return Err(BenchError::Protocol(
                                "diagnostic hit has invalid identity or score".to_string(),
                            ));
                        }
                        if *route == "hybrid" {
                            if hit.contributions.is_empty() || hit.contributions.len() > 2 {
                                return Err(BenchError::Protocol(
                                    "hybrid diagnostic lacks lane provenance".to_string(),
                                ));
                            }
                        } else if !hit.contributions.is_empty() {
                            return Err(BenchError::Protocol(
                                "non-hybrid diagnostic has lane provenance".to_string(),
                            ));
                        }
                        let mut seen = BTreeSet::new();
                        let mut lanes = Vec::with_capacity(hit.contributions.len());
                        for contribution in &hit.contributions {
                            if !matches!(contribution.lane, "lexical" | "dense")
                                || !seen.insert(contribution.lane)
                                || contribution.rank == 0
                                || !contribution.raw_score.is_finite()
                            {
                                return Err(BenchError::Protocol(
                                    "diagnostic has invalid lane contribution".to_string(),
                                ));
                            }
                            lanes.push(json!({
                                "lane": contribution.lane,
                                "rank": contribution.rank,
                                "raw_score": contribution.raw_score,
                            }));
                        }
                        candidates.push(json!({
                            "rank": position + 1,
                            "candidate_id": hit.candidate_id,
                            "path": hit.path,
                            "start_line": hit.start_line,
                            "end_line": hit.end_line,
                            "score": hit.score,
                            "contributions": lanes,
                        }));
                    }
                    (status, Value::Null, candidates)
                }
                QueryOutcome::Failed { status, code, .. } => {
                    if !matches!(*status, "error" | "timeout" | "unavailable") || code.is_empty() {
                        return Err(BenchError::Protocol(
                            "diagnostic failure has invalid status/code".to_string(),
                        ));
                    }
                    (*status, json!(code), Vec::new())
                }
            };
            rows.push(json!({
                "task_id": task.task_id,
                "query_sha256": task.query_sha256,
                "route": route,
                "status": status,
                "error_code": error_code,
                "candidates": candidates,
            }));
        }
    }
    Ok(json!({
        "schema_version": 1,
        "kind": "quanta_returned_window_diagnostic",
        "record_sha256": record_sha256,
        "query_pack_sha256": pack.pack_sha256,
        "top_k": top_k,
        "scope": "returned_window_only",
        "results": rows,
    }))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use quanta_index_contract::ExecutionOutcomeV2;

    use super::*;
    use crate::record::PackTask;
    use crate::sdk::{RankedHit, RankedLaneContribution};

    fn pack() -> QueryPack {
        QueryPack {
            suite_id: "suite".to_string(),
            suite_commitment_sha256: "a".repeat(64),
            repository_commit: "commit".to_string(),
            tokenizer: "qi-regex-v1".to_string(),
            tokenizer_budget_version: Some("qb-v1".to_string()),
            routes: vec!["hybrid".to_string()],
            file_universe: Vec::new(),
            file_universe_digest: "b".repeat(64),
            tasks: vec![PackTask {
                task_id: "T1".to_string(),
                query: "find symbol".to_string(),
                query_sha256: "c".repeat(64),
            }],
            pack_sha256: "d".repeat(64),
            comparison_contract: json!({}),
            contract_top_k: 10,
        }
    }

    fn outcomes() -> BTreeMap<(String, String), QueryOutcome> {
        BTreeMap::from([(
            ("T1".to_string(), "hybrid".to_string()),
            QueryOutcome::Hits {
                hits: vec![RankedHit {
                    candidate_id: "chunk-1".to_string(),
                    path: "src/lib.rs".to_string(),
                    start_line: 4,
                    end_line: 8,
                    snippet: "symbol".to_string(),
                    score: 0.02,
                    contributions: vec![
                        RankedLaneContribution {
                            lane: "lexical",
                            rank: 2,
                            raw_score: 3.0,
                        },
                        RankedLaneContribution {
                            lane: "dense",
                            rank: 5,
                            raw_score: 0.7,
                        },
                    ],
                }],
                outcome: ExecutionOutcomeV2::ExactExhausted,
                latency: Duration::from_millis(3),
            },
        )])
    }

    #[test]
    fn preserves_hybrid_lane_provenance_and_record_binding() {
        let value = diagnostic_value(&"e".repeat(64), &pack(), &["hybrid"], &outcomes(), 10)
            .expect("complete diagnostic");
        assert_eq!(value["record_sha256"], "e".repeat(64));
        assert_eq!(
            value["results"][0]["candidates"][0]["contributions"][0]["rank"],
            2
        );
        assert_eq!(
            value["results"][0]["candidates"][0]["contributions"][1]["lane"],
            "dense"
        );
    }

    #[test]
    fn refuses_missing_result_or_lane_provenance() {
        assert!(
            diagnostic_value(&"e".repeat(64), &pack(), &["hybrid"], &BTreeMap::new(), 10).is_err()
        );
        let mut missing_lane = outcomes();
        let outcome = missing_lane
            .get_mut(&("T1".to_string(), "hybrid".to_string()))
            .expect("fixture result");
        if let QueryOutcome::Hits { hits, .. } = outcome {
            hits[0].contributions.clear();
        }
        assert!(
            diagnostic_value(&"e".repeat(64), &pack(), &["hybrid"], &missing_lane, 10).is_err()
        );
    }
}
