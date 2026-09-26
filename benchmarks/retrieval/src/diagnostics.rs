//! Record-bound, non-scoring SDK retrieval diagnostics.
//!
//! This artifact is deliberately separate from the current v5 runner record.
//! It exposes the returned candidate window and hybrid lane contributions,
//! not the unreturned lane universe. Server timings are included only when
//! the response carries typed stage measurements.

use std::collections::{BTreeMap, BTreeSet};

use quanta_index_search_plane::QueryStageObservationPolicy;
use serde_json::{Value, json};

use crate::record::QueryPack;
use crate::sdk::{QueryOutcome, RouteExplanation};
use crate::{BenchError, BenchResult};

fn lane_trace_matches_contribution(trace_lane: &str, contribution_lane: &str) -> bool {
    trace_lane == contribution_lane || trace_lane.strip_prefix("hybrid.") == Some(contribution_lane)
}

fn explanation_value(explanation: Option<&RouteExplanation>) -> Value {
    let Some(detail) = explanation else {
        return Value::Null;
    };
    json!({
        "request_id": detail.request_id,
        "early_stop_reason": detail.early_stop_reason,
        "engines_executed": detail.engines_executed,
        "engines_touched": detail.engines_touched,
        "strategy": detail.strategy,
        "stage_timings": detail.stage_timings,
    })
}

pub fn diagnostic_value(
    record_sha256: &str,
    record: &Value,
    pack: &QueryPack,
    routes: &[&str],
    outcomes: &BTreeMap<(String, String), QueryOutcome>,
    observation_policy: QueryStageObservationPolicy,
) -> BenchResult<Value> {
    let top_k = pack.contract_top_k;
    if record_sha256.len() != 64
        || !record_sha256
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(BenchError::Protocol(
            "diagnostic record digest must be lowercase sha256".to_string(),
        ));
    }
    if top_k == 0 {
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
            let classification = outcome.classification().map_err(|message| {
                BenchError::Protocol(format!(
                    "diagnostic typed outcome is invalid for {}/{}: {message}",
                    task.task_id, route
                ))
            })?;
            let (candidates, response_kind, response) = match outcome {
                QueryOutcome::ReturnedWindow {
                    hits,
                    window,
                    explanation,
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
                    let record_candidates = record_row
                        .get("candidates")
                        .and_then(Value::as_array)
                        .ok_or_else(|| {
                            BenchError::Protocol("record candidates are malformed".to_string())
                        })?;
                    let record_error_code = record_row
                        .get("error")
                        .and_then(|error| error.get("code"))
                        .and_then(Value::as_str);
                    if record_row.get("status").and_then(Value::as_str)
                        != Some(classification.status)
                        || record_candidates.len() != hits.len()
                        || record_error_code != classification.error_code.as_deref()
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
                            let executed_lanes = window
                                .coverage()
                                .lanes()
                                .iter()
                                .filter(|lane| lane.executed())
                                .map(quanta_index_contract::LaneTraceV1::lane)
                                .collect::<Vec<_>>();
                            if !executed_lanes.iter().any(|lane| {
                                lane_trace_matches_contribution(lane, contribution.lane)
                            }) {
                                return Err(BenchError::Protocol(format!(
                                    "candidate contribution lane {:?} is absent from executed lanes {:?}",
                                    contribution.lane, executed_lanes
                                )));
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
                    (
                        candidates,
                        "returned_window",
                        json!({
                            "window": window,
                            "explanation": explanation_value(explanation.as_ref()),
                        }),
                    )
                }
                QueryOutcome::RejectedResponse {
                    code,
                    observed_hit_count,
                    window,
                    explanation,
                    expected_pin,
                    observed_pin,
                    ..
                } => {
                    debug_assert_eq!(code, "stale_generation");
                    debug_assert_ne!(expected_pin, observed_pin);
                    debug_assert_eq!(u32::try_from(*observed_hit_count), Ok(window.returned()));
                    if record_row.get("status").and_then(Value::as_str) != Some("error")
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
                            "diagnostic rejected response differs from record for {}/{}",
                            task.task_id, route
                        )));
                    }
                    (
                        Vec::new(),
                        "rejected_response",
                        json!({
                            "window": window,
                            "explanation": explanation_value(explanation.as_ref()),
                            "observed_hit_count": observed_hit_count,
                            "expected_generation": expected_pin,
                            "observed_generation": observed_pin,
                        }),
                    )
                }
                QueryOutcome::SdkFailure { status, code, .. } => {
                    debug_assert!(matches!(*status, "error" | "timeout" | "unavailable"));
                    debug_assert!(!code.is_empty());
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
                    (Vec::new(), "sdk_failure", Value::Null)
                }
            };
            rows.push(json!({
                "task_id": task.task_id,
                "query_sha256": task.query_sha256,
                "route": route,
                "status": classification.status,
                "error_code": classification.error_code,
                "candidates": candidates,
                "response_kind": response_kind,
                "response": response,
            }));
        }
    }
    Ok(json!({
        "schema_version": 5,
        "server_observation": server_observation_value(observation_policy)?,
        "ingest": null,
        "kind": "quanta_returned_window_diagnostic",
        "record_sha256": record_sha256,
        "query_pack_sha256": pack.pack_sha256,
        "top_k": top_k,
        "scope": "returned_window_only",
        "results": rows,
    }))
}

/// Canonical startup configuration is bound separately from sidecar output.
/// This scope excludes pre-existing request/deadline and operational metric clocks.
pub fn server_observation_value(policy: QueryStageObservationPolicy) -> BenchResult<Value> {
    let mut config = json!({
        "query_stages": policy.as_str(),
        "scope": "server_query_stage_only_v1",
    });
    let digest = crate::sha256_hex(crate::canonical::canonical_json(&config)?.as_bytes());
    let object = config.as_object_mut().ok_or_else(|| {
        BenchError::Protocol("server observation config is not an object".to_string())
    })?;
    let _previous = object.insert("config_sha256".to_string(), json!(digest));
    Ok(config)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use quanta_index_contract::{
        CandidateCountV1, ExhaustionProofV1, LaneTraceV1, QueryResultWindowV2,
    };

    use super::*;
    use crate::record::PackTask;
    use crate::sdk::{RankedHit, RankedLaneContribution, RouteExplanation};

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

    fn route_explanation_fixture() -> RouteExplanation {
        RouteExplanation {
            request_id: Some(7),
            early_stop_reason: Some("count_reached"),
            engines_executed: Some(vec!["lexical", "semantic"]),
            engines_touched: Some(vec!["lexical", "semantic"]),
            strategy: Some("hybrid-rrf".to_string()),
            stage_timings: None,
        }
    }

    fn window_fixture() -> QueryResultWindowV2 {
        QueryResultWindowV2::exact_exhausted(
            1,
            ExhaustionProofV1::ExactCount { total: 1 },
            vec![
                LaneTraceV1::new("hybrid.lexical", true, true)
                    .with_candidates(CandidateCountV1::Exact(12))
                    .with_filtered_out(3)
                    .with_cost(40)
                    .with_profile("bm25"),
                // Executed and contributed remain independent.
                LaneTraceV1::new("hybrid.dense", true, false)
                    .with_candidates(CandidateCountV1::AtLeast(64)),
            ],
        )
    }

    fn outcomes() -> BTreeMap<(String, String), QueryOutcome> {
        BTreeMap::from([(
            ("T1".to_string(), "hybrid".to_string()),
            QueryOutcome::ReturnedWindow {
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
                window: window_fixture(),
                explanation: Some(route_explanation_fixture()),
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
            QueryStageObservationPolicy::Enabled,
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
            QueryStageObservationPolicy::Enabled,
        )
        .expect("complete diagnostic");
        assert_eq!(value.get("schema_version"), Some(&json!(5)));
        assert_eq!(
            value.pointer("/results/0/response_kind"),
            Some(&json!("returned_window"))
        );
        // Response-level facts survive alongside the candidates.
        assert_eq!(
            value.pointer("/results/0/response/explanation/request_id"),
            Some(&json!(7))
        );
        assert_eq!(
            value.pointer("/results/0/response/explanation/early_stop_reason"),
            Some(&json!("count_reached"))
        );
        assert_eq!(
            value.pointer("/results/0/response/explanation/engines_executed"),
            Some(&json!(["lexical", "semantic"]))
        );
        assert_eq!(
            value.pointer("/results/0/response/window/candidate_count"),
            Some(&json!({"kind": "exact", "value": 1}))
        );
        // An executed lane that contributed nothing stays distinct from a
        // lane that never ran.
        assert_eq!(
            value.pointer("/results/0/response/window/coverage/lanes/0/executed"),
            Some(&json!(true))
        );
        assert_eq!(
            value.pointer("/results/0/response/window/coverage/lanes/0/contributed"),
            Some(&json!(true))
        );
        assert_eq!(
            value.pointer("/results/0/response/window/coverage/lanes/1/lane"),
            Some(&json!("hybrid.dense"))
        );
        assert_eq!(
            value.pointer("/results/0/response/window/coverage/lanes/1/executed"),
            Some(&json!(true))
        );
        assert_eq!(
            value.pointer("/results/0/response/window/coverage/lanes/1/contributed"),
            Some(&json!(false))
        );
        assert_eq!(
            value.pointer("/results/0/response/window/coverage/lanes/1/candidates"),
            Some(&json!({"kind": "at_least", "value": 64}))
        );
    }

    #[test]
    fn missing_response_observations_stay_null_never_defaulted() {
        let mut sparse = outcomes();
        let outcome = sparse
            .get_mut(&("T1".to_string(), "hybrid".to_string()))
            .expect("fixture result");
        if let QueryOutcome::ReturnedWindow { explanation, .. } = outcome {
            *explanation = None;
        }
        let value = diagnostic_value(
            &"e".repeat(64),
            &record(),
            &pack(),
            &["hybrid"],
            &sparse,
            QueryStageObservationPolicy::Disabled,
        )
        .expect("complete diagnostic");
        // Absent observations remain explicit nulls: no explanation, no
        // counts, no fabricated lanes.
        assert_eq!(
            value.pointer("/results/0/response/explanation"),
            Some(&Value::Null)
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
                QueryStageObservationPolicy::Enabled,
            )
            .is_err()
        );
        let mut missing_lane = outcomes();
        let outcome = missing_lane
            .get_mut(&("T1".to_string(), "hybrid".to_string()))
            .expect("fixture result");
        if let QueryOutcome::ReturnedWindow { hits, .. } = outcome {
            hits.first_mut().expect("fixture hit").contributions.clear();
        }
        assert!(
            diagnostic_value(
                &"e".repeat(64),
                &record(),
                &pack(),
                &["hybrid"],
                &missing_lane,
                QueryStageObservationPolicy::Enabled,
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
        if let QueryOutcome::ReturnedWindow { hits, .. } = outcome {
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
            QueryStageObservationPolicy::Enabled,
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
