//! Record-bound, non-scoring SDK retrieval diagnostics.
//!
//! This artifact is deliberately separate from the frozen v3 runner record.
//! It exposes the returned candidate window and hybrid lane contributions,
//! not the unreturned lane universe or server-internal timing.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};

use crate::record::QueryPack;
use crate::sdk::{QueryOutcome, ResponseDetail};
use crate::{BenchError, BenchResult};

fn response_value(detail: &ResponseDetail) -> Value {
    json!({
        // null means the route response carried no explanation or no
        // observation: missing, never a silent empty or zero.
        "request_id": detail.request_id,
        "early_stop_reason": detail.early_stop_reason,
        "engines_executed": detail.engines_executed,
        "engines_touched": detail.engines_touched,
        "strategy": detail.strategy,
        "window_returned": detail.window_returned,
        "window_candidate_count": detail
            .window_candidate_count
            .map(|(kind, count)| json!({"kind": kind, "count": count})),
        "lane_traces": detail
            .lane_traces
            .iter()
            .map(|lane| {
                json!({
                    "lane": lane.lane,
                    "executed": lane.executed,
                    "contributed": lane.contributed,
                    "candidates": lane
                        .candidates
                        .map(|(kind, count)| json!({"kind": kind, "count": count})),
                    "filtered_out": lane.filtered_out,
                    "cost": lane.cost,
                    "profile": lane.profile,
                })
            })
            .collect::<Vec<_>>(),
    })
}

pub fn diagnostic_value(
    record_sha256: &str,
    record: &Value,
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
    let expected_count =
        pack.tasks.len().checked_mul(routes.len()).ok_or_else(|| {
            BenchError::Protocol("diagnostic task/route count overflow".to_string())
        })?;
    if expected.len() != expected_count
        || outcomes.keys().cloned().collect::<BTreeSet<_>>() != expected
    {
        return Err(BenchError::Protocol(
            "diagnostic outcomes are missing, duplicated or unexpected".to_string(),
        ));
    }
    let record_rows = record
        .get("results")
        .and_then(Value::as_array)
        .ok_or_else(|| BenchError::Protocol("diagnostic record results are missing".to_string()))?;
    let mut normalized = BTreeMap::new();
    for row in record_rows {
        let key = (
            row.get("task_id")
                .and_then(Value::as_str)
                .ok_or_else(|| BenchError::Protocol("record task id is malformed".to_string()))?
                .to_string(),
            row.get("route")
                .and_then(Value::as_str)
                .ok_or_else(|| BenchError::Protocol("record route is malformed".to_string()))?
                .to_string(),
        );
        if !expected.contains(&key) || normalized.insert(key, row).is_some() {
            return Err(BenchError::Protocol(
                "diagnostic record results are duplicated or unexpected".to_string(),
            ));
        }
    }
    if normalized.keys().cloned().collect::<BTreeSet<_>>() != expected {
        return Err(BenchError::Protocol(
            "diagnostic record results are incomplete".to_string(),
        ));
    }
    let mut rows = Vec::with_capacity(expected.len());
    for task in &pack.tasks {
        for route in routes {
            let key = (task.task_id.clone(), (*route).to_string());
            let outcome = outcomes.get(&key).ok_or_else(|| {
                BenchError::Protocol("diagnostic outcome disappeared".to_string())
            })?;
            let record_row = normalized.get(&key).copied().ok_or_else(|| {
                BenchError::Protocol("diagnostic record result disappeared".to_string())
            })?;
            let (status, error_code, candidates, response) = match outcome {
                QueryOutcome::Hits {
                    hits,
                    outcome,
                    detail,
                    ..
                } => {
                    let top_k_len = usize::try_from(top_k).map_err(|error| {
                        BenchError::Protocol(format!(
                            "diagnostic top_k is not addressable: {error}"
                        ))
                    })?;
                    if hits.len() > top_k_len {
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
                    let record_candidates = record_row
                        .get("candidates")
                        .and_then(Value::as_array)
                        .ok_or_else(|| {
                            BenchError::Protocol("record candidates are malformed".to_string())
                        })?;
                    if record_row.get("status").and_then(Value::as_str) != Some(status)
                        || record_candidates.len() != hits.len()
                    {
                        return Err(BenchError::Protocol(format!(
                            "diagnostic outcome differs from record for {}/{}",
                            task.task_id, route
                        )));
                    }
                    let mut candidates = Vec::with_capacity(hits.len());
                    for (position, hit) in hits.iter().enumerate() {
                        if !hit.score.is_finite() || hit.candidate_id.is_empty() {
                            return Err(BenchError::Protocol(
                                "diagnostic hit has invalid identity or score".to_string(),
                            ));
                        }
                        let scored = record_candidates.get(position).ok_or_else(|| {
                            BenchError::Protocol("record candidate disappeared".to_string())
                        })?;
                        let rank = position.checked_add(1).ok_or_else(|| {
                            BenchError::Protocol("diagnostic rank overflow".to_string())
                        })?;
                        let path = scored.get("path").and_then(Value::as_str).ok_or_else(|| {
                            BenchError::Protocol("record candidate path is malformed".to_string())
                        })?;
                        let start_line_value = scored
                            .get("start_line")
                            .and_then(Value::as_u64)
                            .ok_or_else(|| {
                                BenchError::Protocol(
                                    "record candidate start line is malformed".to_string(),
                                )
                            })?;
                        let start_line = u32::try_from(start_line_value).map_err(|error| {
                            BenchError::Protocol(format!(
                                "record candidate start line is out of range: {error}"
                            ))
                        })?;
                        if start_line == 0 {
                            return Err(BenchError::Protocol(
                                "record candidate start line is zero".to_string(),
                            ));
                        }
                        let end_line_value = scored
                            .get("end_line")
                            .and_then(Value::as_u64)
                            .ok_or_else(|| {
                                BenchError::Protocol(
                                    "record candidate end line is malformed".to_string(),
                                )
                            })?;
                        let end_line = u32::try_from(end_line_value).map_err(|error| {
                            BenchError::Protocol(format!(
                                "record candidate end line is out of range: {error}"
                            ))
                        })?;
                        if end_line < start_line {
                            return Err(BenchError::Protocol(
                                "record candidate end line precedes start line".to_string(),
                            ));
                        }
                        let rank_u64 = u64::try_from(rank).map_err(|error| {
                            BenchError::Protocol(format!(
                                "diagnostic rank is out of range: {error}"
                            ))
                        })?;
                        if scored.get("rank").and_then(Value::as_u64) != Some(rank_u64)
                            || path != hit.path
                        {
                            return Err(BenchError::Protocol(format!(
                                "diagnostic candidate identity differs from record for {}/{}",
                                task.task_id, route
                            )));
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
                            "rank": rank,
                            "candidate_id": hit.candidate_id,
                            "path": path,
                            "start_line": start_line,
                            "end_line": end_line,
                            "score": hit.score,
                            "contributions": lanes,
                        }));
                    }
                    (status, Value::Null, candidates, response_value(detail))
                }
                QueryOutcome::Failed { status, code, .. } => {
                    if !matches!(*status, "error" | "timeout" | "unavailable") || code.is_empty() {
                        return Err(BenchError::Protocol(
                            "diagnostic failure has invalid status/code".to_string(),
                        ));
                    }
                    if record_row.get("status").and_then(Value::as_str) != Some(status)
                        || record_row
                            .get("candidates")
                            .and_then(Value::as_array)
                            .is_none_or(|candidates| !candidates.is_empty())
                        || record_row
                            .get("error")
                            .and_then(|error| error.get("code"))
                            .and_then(Value::as_str)
                            != Some(code)
                    {
                        return Err(BenchError::Protocol(format!(
                            "diagnostic failure differs from record for {}/{}",
                            task.task_id, route
                        )));
                    }
                    (*status, json!(code), Vec::new(), Value::Null)
                }
            };
            rows.push(json!({
                "task_id": task.task_id,
                "query_sha256": task.query_sha256,
                "route": route,
                "status": status,
                "error_code": error_code,
                "candidates": candidates,
                "response": response,
            }));
        }
    }
    Ok(json!({
        "schema_version": 2,
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
    use crate::sdk::{LaneTraceFact, RankedHit, RankedLaneContribution, ResponseDetail};

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

    fn response_detail() -> ResponseDetail {
        ResponseDetail {
            request_id: Some(7),
            early_stop_reason: Some("count_reached"),
            engines_executed: Some(vec!["lexical", "semantic"]),
            engines_touched: Some(vec!["lexical", "semantic"]),
            strategy: Some("hybrid-rrf".to_string()),
            window_returned: Some(1),
            window_candidate_count: Some(("exact", 1)),
            lane_traces: vec![
                LaneTraceFact {
                    lane: "lexical",
                    executed: true,
                    contributed: true,
                    candidates: Some(("exact", 12)),
                    filtered_out: Some(3),
                    cost: Some(40),
                    profile: Some("bm25".to_string()),
                },
                // An executed zero-hit lane stays recorded: executed and
                // contributed are independent facts (RBR-01).
                LaneTraceFact {
                    lane: "dense",
                    executed: true,
                    contributed: false,
                    candidates: Some(("at_least", 64)),
                    filtered_out: Some(0),
                    cost: None,
                    profile: None,
                },
            ],
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
                detail: response_detail(),
                latency: Duration::from_millis(3),
            },
        )])
    }

    fn record() -> Value {
        json!({
            "results": [{
                "task_id": "T1",
                "route": "hybrid",
                "status": "success",
                "candidates": [{
                    "rank": 1,
                    "path": "src/lib.rs",
                    "start_line": 4,
                    "end_line": 8,
                }]
            }]
        })
    }

    #[test]
    fn preserves_hybrid_lane_provenance_and_record_binding() {
        let value = diagnostic_value(
            &"e".repeat(64),
            &record(),
            &pack(),
            &["hybrid"],
            &outcomes(),
            10,
        )
        .expect("complete diagnostic");
        assert_eq!(value.get("record_sha256"), Some(&json!("e".repeat(64))));
        assert_eq!(
            value.pointer("/results/0/candidates/0/contributions/0/rank"),
            Some(&json!(2))
        );
        assert_eq!(
            value.pointer("/results/0/candidates/0/contributions/1/lane"),
            Some(&json!("dense"))
        );
    }

    #[test]
    fn preserves_executed_vs_contributed_lanes_and_window_counts() {
        let value = diagnostic_value(
            &"e".repeat(64),
            &record(),
            &pack(),
            &["hybrid"],
            &outcomes(),
            10,
        )
        .expect("complete diagnostic");
        assert_eq!(value.get("schema_version"), Some(&json!(2)));
        // Response-level facts survive alongside the candidates.
        assert_eq!(
            value.pointer("/results/0/response/request_id"),
            Some(&json!(7))
        );
        assert_eq!(
            value.pointer("/results/0/response/early_stop_reason"),
            Some(&json!("count_reached"))
        );
        assert_eq!(
            value.pointer("/results/0/response/engines_executed"),
            Some(&json!(["lexical", "semantic"]))
        );
        assert_eq!(
            value.pointer("/results/0/response/window_candidate_count"),
            Some(&json!({"kind": "exact", "count": 1}))
        );
        // An executed lane that contributed nothing stays distinct from a
        // lane that never ran.
        assert_eq!(
            value.pointer("/results/0/response/lane_traces/0/executed"),
            Some(&json!(true))
        );
        assert_eq!(
            value.pointer("/results/0/response/lane_traces/0/contributed"),
            Some(&json!(true))
        );
        assert_eq!(
            value.pointer("/results/0/response/lane_traces/1/lane"),
            Some(&json!("dense"))
        );
        assert_eq!(
            value.pointer("/results/0/response/lane_traces/1/executed"),
            Some(&json!(true))
        );
        assert_eq!(
            value.pointer("/results/0/response/lane_traces/1/contributed"),
            Some(&json!(false))
        );
        assert_eq!(
            value.pointer("/results/0/response/lane_traces/1/candidates"),
            Some(&json!({"kind": "at_least", "count": 64}))
        );
    }

    #[test]
    fn missing_response_observations_stay_null_never_defaulted() {
        let mut sparse = outcomes();
        let outcome = sparse
            .get_mut(&("T1".to_string(), "hybrid".to_string()))
            .expect("fixture result");
        if let QueryOutcome::Hits { detail, .. } = outcome {
            *detail = ResponseDetail::default();
        }
        let value = diagnostic_value(
            &"e".repeat(64),
            &record(),
            &pack(),
            &["hybrid"],
            &sparse,
            10,
        )
        .expect("complete diagnostic");
        // Absent observations remain explicit nulls: no explanation, no
        // counts, no fabricated lanes.
        assert_eq!(
            value.pointer("/results/0/response/engines_executed"),
            Some(&Value::Null)
        );
        assert_eq!(
            value.pointer("/results/0/response/request_id"),
            Some(&Value::Null)
        );
        assert_eq!(
            value.pointer("/results/0/response/lane_traces"),
            Some(&json!([]))
        );
    }

    #[test]
    fn refuses_missing_result_or_lane_provenance() {
        assert!(
            diagnostic_value(
                &"e".repeat(64),
                &record(),
                &pack(),
                &["hybrid"],
                &BTreeMap::new(),
                10,
            )
            .is_err()
        );
        let mut missing_lane = outcomes();
        let outcome = missing_lane
            .get_mut(&("T1".to_string(), "hybrid".to_string()))
            .expect("fixture result");
        if let QueryOutcome::Hits { hits, .. } = outcome {
            hits.first_mut().expect("fixture hit").contributions.clear();
        }
        assert!(
            diagnostic_value(
                &"e".repeat(64),
                &record(),
                &pack(),
                &["hybrid"],
                &missing_lane,
                10,
            )
            .is_err()
        );
    }

    #[test]
    fn uses_record_span_when_sdk_hit_is_unanchored() {
        let mut unanchored = outcomes();
        let outcome = unanchored
            .get_mut(&("T1".to_string(), "hybrid".to_string()))
            .expect("fixture result");
        if let QueryOutcome::Hits { hits, .. } = outcome {
            let hit = hits.first_mut().expect("fixture hit");
            hit.start_line = 0;
            hit.end_line = 0;
        }
        let value = diagnostic_value(
            &"e".repeat(64),
            &record(),
            &pack(),
            &["hybrid"],
            &unanchored,
            10,
        )
        .expect("record supplies the normalized span");
        assert_eq!(
            value.pointer("/results/0/candidates/0/start_line"),
            Some(&json!(4))
        );
        assert_eq!(
            value.pointer("/results/0/candidates/0/end_line"),
            Some(&json!(8))
        );
    }
}
