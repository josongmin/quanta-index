//! Record-bound, non-scoring SDK retrieval diagnostics.
//!
//! This artifact is deliberately separate from the current v5 runner record.
//! It exposes the returned candidate window and hybrid lane contributions,
//! not the unreturned lane universe. Server timings are included only when
//! the response carries typed stage measurements.

use std::collections::{BTreeMap, BTreeSet};

use quanta_index_contract::QueryResultWindowV2;
use quanta_index_search_plane::{HybridFetchFloorPolicy, QueryStageObservationPolicy};
use serde_json::{Value, json};

use crate::record::{NativeSpanProof, QueryPack};
use crate::sdk::{QueryOutcome, RankedHit, RouteExplanation};
use crate::{BenchError, BenchResult};

/// Current diagnostic artifact version, independent of runner record schema.
pub const DIAGNOSTIC_SCHEMA_VERSION: u64 = 7;

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
        "planner_trace": detail.planner_trace,
        "stage_timings": detail.stage_timings,
    })
}

fn proven_lanes(
    hit: &RankedHit,
    route: &str,
    window: &QueryResultWindowV2,
) -> BenchResult<Vec<Value>> {
    if !hit.score.is_finite() || hit.candidate_id.is_empty() {
        return Err(BenchError::Protocol(
            "diagnostic hit has invalid identity or score".to_string(),
        ));
    }
    if route == "hybrid" {
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
        if !executed_lanes
            .iter()
            .any(|lane| lane_trace_matches_contribution(lane, contribution.lane))
        {
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
    Ok(lanes)
}

pub fn diagnostic_value(
    record_sha256: &str,
    record: &Value,
    pack: &QueryPack,
    routes: &[&str],
    outcomes: &BTreeMap<(String, String), QueryOutcome>,
    observation_policy: QueryStageObservationPolicy,
    fetch_floor_policy: HybridFetchFloorPolicy,
    native_spans: &BTreeMap<(String, String), Vec<NativeSpanProof>>,
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
                        || record_candidates.len() > hits.len()
                        || record_candidates.is_empty() != hits.is_empty()
                        || record_error_code != classification.error_code.as_deref()
                    {
                        return Err(BenchError::Protocol(format!(
                            "diagnostic outcome differs from record for {}/{}",
                            task.task_id, route
                        )));
                    }
                    // Legacy records keep the first unit per proven source
                    // span; bound symbol ranks preserve each identity. The
                    // typed window counts every native hit. Validate all
                    // native units, including those not separately scored.
                    let mut hit_positions = BTreeMap::new();
                    let mut hit_lanes = Vec::with_capacity(hits.len());
                    for (position, hit) in hits.iter().enumerate() {
                        hit_lanes.push(proven_lanes(hit, route, window)?);
                        if hit_positions
                            .insert(hit.candidate_id.as_str(), position)
                            .is_some()
                        {
                            return Err(BenchError::Protocol(
                                "diagnostic SDK hits reuse a published unit ID".to_string(),
                            ));
                        }
                    }
                    let mut candidates = Vec::with_capacity(record_candidates.len());
                    let mut previous_hit_position = None;
                    for (position, scored) in record_candidates.iter().enumerate() {
                        let unit_id = scored
                            .pointer("/span_accounting/unit_id")
                            .and_then(Value::as_str)
                            .ok_or_else(|| {
                                BenchError::Protocol(
                                    "record candidate lacks a published unit ID".to_string(),
                                )
                            })?;
                        let hit_position = *hit_positions.get(unit_id).ok_or_else(|| {
                            BenchError::Protocol(
                                "record candidate unit is absent from SDK hits".to_string(),
                            )
                        })?;
                        if previous_hit_position.is_some_and(|previous| hit_position <= previous) {
                            return Err(BenchError::Protocol(
                                "record candidate order differs from SDK hits".to_string(),
                            ));
                        }
                        previous_hit_position = Some(hit_position);
                        let hit = hits.get(hit_position).ok_or_else(|| {
                            BenchError::Protocol("diagnostic SDK hit disappeared".to_string())
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
                        let lanes = hit_lanes.get(hit_position).ok_or_else(|| {
                            BenchError::Protocol("diagnostic hit lanes disappeared".to_string())
                        })?;
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
                    let mut response = json!({
                        "window": window,
                        "explanation": explanation_value(explanation.as_ref()),
                    });
                    if record_row.get("rank_unit").and_then(Value::as_str) == Some("symbol") {
                        if *route != "symbol" || record_candidates.len() != hits.len() {
                            return Err(BenchError::Protocol(
                                "symbol rank requires one scored row per native unit".into(),
                            ));
                        }
                        let spans = native_spans.get(&key).ok_or_else(|| {
                            BenchError::Protocol("symbol diagnostic lacks source proofs".into())
                        })?;
                        if spans.len() != hits.len() {
                            return Err(BenchError::Protocol(
                                "symbol source proof count differs from hits".into(),
                            ));
                        }
                        let mut projected = Vec::with_capacity(spans.len());
                        for (position, ((proof, hit), scored)) in
                            spans.iter().zip(hits).zip(record_candidates).enumerate()
                        {
                            let rank = position.checked_add(1).ok_or_else(|| {
                                BenchError::Protocol("symbol projection rank overflow".into())
                            })?;
                            if proof.unit_kind != crate::published_units::PublishedUnitKind::Symbol
                                || proof.unit_id != hit.candidate_id
                                || proof.span.0 != hit.path
                                || scored
                                    .pointer("/span_accounting/unit_kind")
                                    .and_then(Value::as_str)
                                    != Some("symbol")
                                || scored
                                    .pointer("/span_accounting/unit_id")
                                    .and_then(Value::as_str)
                                    != Some(proof.unit_id.as_str())
                                || scored.get("path").and_then(Value::as_str)
                                    != Some(proof.span.0.as_str())
                                || scored.get("start_byte").and_then(Value::as_u64)
                                    != Some(proof.span.1)
                                || scored.get("end_byte").and_then(Value::as_u64)
                                    != Some(proof.span.2)
                                || scored
                                    .pointer("/span_accounting/indexed_start_byte")
                                    .and_then(Value::as_u64)
                                    != Some(u64::from(proof.indexed_span.0))
                                || scored
                                    .pointer("/span_accounting/indexed_end_byte")
                                    .and_then(Value::as_u64)
                                    != Some(u64::from(proof.indexed_span.1))
                            {
                                return Err(BenchError::Protocol("symbol record differs from native published identity and source span".into()));
                            }
                            projected.push(json!({"candidate_id": proof.unit_id, "path": proof.span.0, "start_byte": proof.span.1, "end_byte": proof.span.2, "scored_rank": rank}));
                        }
                        let _previous = response
                            .as_object_mut()
                            .ok_or_else(|| {
                                BenchError::Protocol("diagnostic response is not an object".into())
                            })?
                            .insert(
                                "native_projection".to_string(),
                                json!({"policy": "symbol-unit-v1", "hits": projected}),
                            );
                    } else if record_candidates.len() != hits.len() {
                        let spans = native_spans.get(&key).ok_or_else(|| {
                            BenchError::Protocol("collapsed diagnostic lacks source proofs".into())
                        })?;
                        if spans.len() != hits.len() {
                            return Err(BenchError::Protocol(
                                "native source proof count differs from hits".into(),
                            ));
                        }
                        let mut scored_spans = BTreeMap::new();
                        for (position, scored) in record_candidates.iter().enumerate() {
                            let path =
                                scored.get("path").and_then(Value::as_str).ok_or_else(|| {
                                    BenchError::Protocol("scored path is malformed".into())
                                })?;
                            let start = scored
                                .get("start_byte")
                                .and_then(Value::as_u64)
                                .ok_or_else(|| {
                                    BenchError::Protocol("scored start byte is malformed".into())
                                })?;
                            let end = scored.get("end_byte").and_then(Value::as_u64).ok_or_else(
                                || BenchError::Protocol("scored end byte is malformed".into()),
                            )?;
                            let rank = position.checked_add(1).ok_or_else(|| {
                                BenchError::Protocol("scored projection rank overflow".into())
                            })?;
                            if scored_spans
                                .insert((path.to_string(), start, end), rank)
                                .is_some()
                            {
                                return Err(BenchError::Protocol(
                                    "scored source spans are duplicated".into(),
                                ));
                            }
                        }
                        let mut projected = Vec::with_capacity(spans.len());
                        let mut first_units = BTreeMap::new();
                        for (proof, hit) in spans.iter().zip(hits) {
                            if proof.unit_id != hit.candidate_id || proof.span.0 != hit.path {
                                return Err(BenchError::Protocol(
                                    "native source proof differs from hit".into(),
                                ));
                            }
                            let rank = *scored_spans.get(&proof.span).ok_or_else(|| {
                                BenchError::Protocol(
                                    "native source span was omitted from scored record".into(),
                                )
                            })?;
                            if !first_units.contains_key(&rank) {
                                let expected = rank
                                    .checked_sub(1)
                                    .and_then(|position| record_candidates.get(position))
                                    .and_then(|scored| scored.pointer("/span_accounting/unit_id"))
                                    .and_then(Value::as_str);
                                if expected != Some(proof.unit_id.as_str())
                                    || first_units.len().checked_add(1) != Some(rank)
                                {
                                    return Err(BenchError::Protocol(
                                        "scored record differs from first native source spans"
                                            .into(),
                                    ));
                                }
                                let _previous = first_units.insert(rank, proof.unit_id.as_str());
                            }
                            projected.push(json!({"candidate_id": proof.unit_id, "path": proof.span.0, "start_byte": proof.span.1, "end_byte": proof.span.2, "scored_rank": rank}));
                        }
                        let object = response.as_object_mut().ok_or_else(|| {
                            BenchError::Protocol("diagnostic response is not an object".into())
                        })?;
                        let _previous = object.insert(
                            "native_projection".to_string(),
                            json!({"policy": "first-source-span-v1", "hits": projected}),
                        );
                    }
                    (candidates, "returned_window", response)
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
        "schema_version": DIAGNOSTIC_SCHEMA_VERSION,
        "server_observation": server_observation_value(observation_policy)?,
        "hybrid_fetch_policy": hybrid_fetch_policy_value(fetch_floor_policy)?,
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

/// Bind the explicit experimental fetch floor without changing product defaults.
pub fn hybrid_fetch_policy_value(policy: HybridFetchFloorPolicy) -> BenchResult<Value> {
    let mut config = json!({
        "floor": policy.get(),
        "scope": "experimental_hybrid_fetch_floor_v1",
    });
    let digest = crate::sha256_hex(crate::canonical::canonical_json(&config)?.as_bytes());
    let object = config
        .as_object_mut()
        .ok_or_else(|| BenchError::Protocol("hybrid fetch config is not an object".to_string()))?;
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
            planner_trace: None,
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
        .expect("valid exact diagnostics fixture")
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
                    file_authority: None,
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
                    "span_accounting": {"unit_id": "chunk-1"},
                    "start_byte": 0,
                    "end_byte": 10,
                }]
            }]
        })
    }

    fn projection_fixture() -> BTreeMap<(String, String), Vec<crate::record::NativeSpanProof>> {
        BTreeMap::from([(
            ("T1".to_string(), "hybrid".to_string()),
            vec![
                crate::record::NativeSpanProof {
                    unit_id: "chunk-1".to_string(),
                    span: ("src/lib.rs".to_string(), 0, 10),
                    unit_kind: crate::published_units::PublishedUnitKind::Chunk,
                    indexed_span: (0, 10),
                },
                crate::record::NativeSpanProof {
                    unit_id: "chunk-2".to_string(),
                    span: ("src/lib.rs".to_string(), 0, 10),
                    unit_kind: crate::published_units::PublishedUnitKind::Chunk,
                    indexed_span: (0, 10),
                },
            ],
        )])
    }

    #[test]
    fn symbol_unit_projection_preserves_same_line_identities_and_refuses_tampering() {
        let mut pack = pack();
        pack.routes = vec!["symbol".to_string()];
        let ids = ["symbol-a", "symbol-b"];
        let hits: Vec<_> = ids
            .iter()
            .map(|id| RankedHit {
                candidate_id: (*id).to_string(),
                path: "src/lib.rs".to_string(),
                start_line: 1,
                end_line: 1,
                snippet: "same-line symbol".to_string(),
                score: 1.0,
                file_authority: None,
                contributions: Vec::new(),
            })
            .collect();
        let key = ("T1".to_string(), "symbol".to_string());
        let outcomes = BTreeMap::from([(
            key.clone(),
            QueryOutcome::ReturnedWindow {
                hits,
                window: QueryResultWindowV2::exact_probe(2),
                explanation: None,
                latency: Duration::from_millis(1),
            },
        )]);
        let proofs = BTreeMap::from([(
            key,
            vec![
                NativeSpanProof {
                    unit_id: "symbol-a".to_string(),
                    span: ("src/lib.rs".to_string(), 0, 30),
                    unit_kind: crate::published_units::PublishedUnitKind::Symbol,
                    indexed_span: (0, 10),
                },
                NativeSpanProof {
                    unit_id: "symbol-b".to_string(),
                    span: ("src/lib.rs".to_string(), 0, 30),
                    unit_kind: crate::published_units::PublishedUnitKind::Symbol,
                    indexed_span: (12, 25),
                },
            ],
        )]);
        let record = json!({"results": [{
            "task_id": "T1", "route": "symbol", "status": "success", "rank_unit": "symbol",
            "candidates": [
                {"rank": 1, "path": "src/lib.rs", "start_line": 1, "end_line": 1,
                 "start_byte": 0, "end_byte": 30,
                 "span_accounting": {"unit_id": "symbol-a", "unit_kind": "symbol", "indexed_start_byte": 0, "indexed_end_byte": 10}},
                {"rank": 2, "path": "src/lib.rs", "start_line": 1, "end_line": 1,
                 "start_byte": 0, "end_byte": 30,
                 "span_accounting": {"unit_id": "symbol-b", "unit_kind": "symbol", "indexed_start_byte": 12, "indexed_end_byte": 25}},
            ]
        }]});
        let diagnose = |record: &Value| {
            diagnostic_value(
                &"e".repeat(64),
                record,
                &pack,
                &["symbol"],
                &outcomes,
                QueryStageObservationPolicy::Disabled,
                HybridFetchFloorPolicy::default(),
                &proofs,
            )
        };
        let diagnostic = diagnose(&record).expect("same line has two independent symbol ranks");
        assert_eq!(
            diagnostic.pointer("/results/0/response/native_projection/policy"),
            Some(&json!("symbol-unit-v1"))
        );
        let projected = diagnostic
            .pointer("/results/0/response/native_projection/hits")
            .and_then(Value::as_array)
            .expect("projected symbols");
        assert_eq!(
            projected.as_slice(),
            &[
                json!({"candidate_id": "symbol-a", "path": "src/lib.rs", "start_byte": 0, "end_byte": 30, "scored_rank": 1}),
                json!({"candidate_id": "symbol-b", "path": "src/lib.rs", "start_byte": 0, "end_byte": 30, "scored_rank": 2}),
            ]
        );
        for (pointer, forged) in [
            (
                "/results/0/candidates/1/span_accounting/unit_id",
                json!("symbol-a"),
            ),
            ("/results/0/candidates/1/rank", json!(1)),
            (
                "/results/0/candidates/1/span_accounting/indexed_start_byte",
                json!(13),
            ),
        ] {
            let mut changed = record.clone();
            *changed.pointer_mut(pointer).expect("fixture field") = forged;
            assert!(diagnose(&changed).is_err(), "tamper must refuse: {pointer}");
        }
        let mut legacy = record.clone();
        let _removed_unit = legacy
            .pointer_mut("/results/0")
            .expect("record row")
            .as_object_mut()
            .expect("record row")
            .remove("rank_unit");
        let _removed_candidate = legacy
            .pointer_mut("/results/0/candidates")
            .expect("candidates")
            .as_array_mut()
            .expect("candidates")
            .pop()
            .expect("second scored candidate");
        let legacy_diagnostic =
            diagnose(&legacy).expect("Native symbol context scoring stays valid");
        assert_eq!(
            legacy_diagnostic.pointer("/results/0/response/native_projection"),
            Some(&json!({"policy": "first-source-span-v1", "hits": [
                {"candidate_id": "symbol-a", "path": "src/lib.rs", "start_byte": 0, "end_byte": 30, "scored_rank": 1},
                {"candidate_id": "symbol-b", "path": "src/lib.rs", "start_byte": 0, "end_byte": 30, "scored_rank": 1}
            ]}))
        );
        assert_eq!(
            legacy_diagnostic.pointer("/results/0/response/window/returned"),
            Some(&json!(2))
        );
        assert_eq!(
            legacy_diagnostic
                .pointer("/results/0/candidates")
                .and_then(Value::as_array)
                .expect("scored rows")
                .len(),
            1
        );
        let mut dropped = record;
        let _removed = dropped
            .pointer_mut("/results/0/candidates")
            .and_then(Value::as_array_mut)
            .expect("candidates")
            .pop();
        assert!(
            diagnose(&dropped).is_err(),
            "symbol units cannot collapse by context"
        );
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
            HybridFetchFloorPolicy::default(),
            &projection_fixture(),
        )
        .expect("complete diagnostic");
        assert_eq!(value.get("record_sha256"), Some(&json!("e".repeat(64))));
        for (policy, floor, digest) in [
            (
                HybridFetchFloorPolicy::Floor25,
                25,
                "cb0a17b99da23421f29d5aced57dbf590a8f5b3d4b3fd6e1a38f34be43bc84ce",
            ),
            (
                HybridFetchFloorPolicy::Floor50,
                50,
                "4ae57a1c4b08695816cd19ba3bdbaae560ff2b6ed3e07ba936b570253b641c91",
            ),
            (
                HybridFetchFloorPolicy::Floor100,
                100,
                "e874086df4989a03131d54376525577cd3549d214ecff199f99f663b8a1f6e2c",
            ),
        ] {
            assert_eq!(
                hybrid_fetch_policy_value(policy).expect("config"),
                json!({
                    "floor": floor, "scope": "experimental_hybrid_fetch_floor_v1",
                    "config_sha256": digest,
                })
            );
        }
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
    fn duplicate_scored_span_keeps_first_native_provenance_and_raw_window_count() {
        let mut raw = outcomes();
        let outcome = raw
            .get_mut(&("T1".to_string(), "hybrid".to_string()))
            .expect("fixture result");
        if let QueryOutcome::ReturnedWindow { hits, window, .. } = outcome {
            let mut duplicate = hits.first().expect("first fixture hit").clone();
            duplicate.candidate_id = "chunk-2".to_string();
            duplicate.score = 0.01;
            duplicate
                .contributions
                .first_mut()
                .expect("fixture contribution")
                .rank = 7;
            hits.push(duplicate);
            *window = QueryResultWindowV2::exact_exhausted(
                2,
                ExhaustionProofV1::ExactCount { total: 2 },
                vec![
                    LaneTraceV1::new("hybrid.lexical", true, true),
                    LaneTraceV1::new("hybrid.dense", true, true),
                ],
            )
            .expect("two returned hits");
        }
        let diagnostic = diagnostic_value(
            &"e".repeat(64),
            &record(),
            &pack(),
            &["hybrid"],
            &raw,
            QueryStageObservationPolicy::Enabled,
            HybridFetchFloorPolicy::default(),
            &projection_fixture(),
        )
        .expect("two native units project to one scored span");
        assert_eq!(
            diagnostic
                .pointer("/results/0/candidates")
                .expect("candidates")
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            diagnostic
                .pointer("/results/0/candidates/0/candidate_id")
                .expect("fixture field"),
            "chunk-1"
        );
        assert_eq!(
            diagnostic
                .pointer("/results/0/candidates/0/contributions/0/rank")
                .expect("fixture field"),
            2
        );
        assert_eq!(
            diagnostic
                .pointer("/results/0/response/window/returned")
                .expect("fixture field"),
            2
        );
        assert_eq!(
            diagnostic
                .pointer("/results/0/response/native_projection")
                .expect("fixture field"),
            &json!({
                "policy": "first-source-span-v1",
                "hits": [
                    {"candidate_id": "chunk-1", "path": "src/lib.rs", "start_byte": 0, "end_byte": 10, "scored_rank": 1},
                    {"candidate_id": "chunk-2", "path": "src/lib.rs", "start_byte": 0, "end_byte": 10, "scored_rank": 1},
                ],
            })
        );
        let mut substituted = projection_fixture();
        substituted
            .values_mut()
            .next()
            .expect("fixture proofs")
            .get_mut(1)
            .expect("second proof")
            .span
            .2 = 11;
        for proofs in [&BTreeMap::new(), &substituted] {
            assert!(
                diagnostic_value(
                    &"e".repeat(64),
                    &record(),
                    &pack(),
                    &["hybrid"],
                    &raw,
                    QueryStageObservationPolicy::Enabled,
                    HybridFetchFloorPolicy::default(),
                    proofs,
                )
                .is_err()
            );
        }

        if let QueryOutcome::ReturnedWindow { hits, .. } = raw
            .get_mut(&("T1".to_string(), "hybrid".to_string()))
            .expect("fixture result")
        {
            hits.get_mut(1)
                .expect("second hit")
                .contributions
                .first_mut()
                .expect("fixture contribution")
                .raw_score = f32::NAN;
        }
        assert!(
            diagnostic_value(
                &"e".repeat(64),
                &record(),
                &pack(),
                &["hybrid"],
                &raw,
                QueryStageObservationPolicy::Enabled,
                HybridFetchFloorPolicy::default(),
                &projection_fixture(),
            )
            .is_err()
        );

        if let QueryOutcome::ReturnedWindow { hits, .. } = raw
            .get_mut(&("T1".to_string(), "hybrid".to_string()))
            .expect("fixture result")
        {
            hits.get_mut(1)
                .expect("second hit")
                .contributions
                .first_mut()
                .expect("fixture contribution")
                .raw_score = 3.0;
            hits.get_mut(1).expect("second hit").candidate_id = "chunk-1".to_string();
        }
        let error = diagnostic_value(
            &"e".repeat(64),
            &record(),
            &pack(),
            &["hybrid"],
            &raw,
            QueryStageObservationPolicy::Enabled,
            HybridFetchFloorPolicy::default(),
            &projection_fixture(),
        )
        .expect_err("duplicate native unit must not disappear behind span projection");
        assert!(error.to_string().contains("reuse a published unit ID"));
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
            HybridFetchFloorPolicy::default(),
            &projection_fixture(),
        )
        .expect("complete diagnostic");
        assert_eq!(value.get("schema_version"), Some(&json!(7)));
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
            HybridFetchFloorPolicy::default(),
            &projection_fixture(),
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
                HybridFetchFloorPolicy::default(),
                &projection_fixture(),
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
                HybridFetchFloorPolicy::default(),
                &projection_fixture(),
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
            HybridFetchFloorPolicy::default(),
            &projection_fixture(),
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
