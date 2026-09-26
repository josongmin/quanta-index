//! Stage observation is not query authority: every non-observation field and
//! typed failure must survive toggling the startup policy.

use std::sync::{Arc, Mutex};

use quanta_index_contract::{
    HybridQueryRequest, QueryConstraintSetV1, SearchPlaneQueryIpcRequest, SemanticQueryRequest,
    TextQueryRequest, TextQuerySyntax,
};
use quanta_index_core::{CoreError, RequestBudgetV1, RequestCorrelationV1};

use super::support::common::{TestResult, candidate, dispatcher_with_obs, ready_pin};
use super::support::lexical::StubLexicalOpener;
use super::support::semantic::{RecordingSemanticOpener, RecordingSemanticState};
use crate::observability::BoundedQueryObsStore;
use crate::query_dispatcher::continuation::{CursorAuthorityV2, CursorClockV2};

struct FixedCursorClock;
impl CursorClockV2 for FixedCursorClock {
    fn now_unix(&self) -> Result<u64, CoreError> {
        Ok(1_000_000_000)
    }
}
use crate::{QueryStageObservationPolicy, ResponsePayloadBudget};

fn request_text() -> TextQueryRequest {
    TextQueryRequest {
        syntax: TextQuerySyntax::Native,
        query_text: "scope".to_string(),
        constraints: QueryConstraintSetV1::unconstrained(),
        generation: Some(ready_pin()),
        generation_selector: None,
        top_k: 10,
        cursor: None,
    }
}

fn compare_policy(request: SearchPlaneQueryIpcRequest) -> TestResult {
    compare_policy_with_budget(request, ResponsePayloadBudget::DEFAULT, false)
}

fn compare_policy_with_budget(
    request: SearchPlaneQueryIpcRequest,
    payload_budget: ResponsePayloadBudget,
    cancelled: bool,
) -> TestResult {
    compare_policy_with_rows(
        request,
        payload_budget,
        cancelled,
        vec![candidate("lex-a", 1.0)],
    )
}

fn compare_policy_with_rows(
    request: SearchPlaneQueryIpcRequest,
    payload_budget: ResponsePayloadBudget,
    cancelled: bool,
    rows: Vec<quanta_index_contract::LexicalCandidate>,
) -> TestResult {
    let expected_uncut_rows = rows.len();
    let dispatcher = dispatcher_with_obs(
        Arc::new(StubLexicalOpener { results: rows }),
        Arc::new(RecordingSemanticOpener {
            state: Arc::new(Mutex::new(RecordingSemanticState::default())),
        }),
        Arc::new(BoundedQueryObsStore::default()),
    )?
    .with_response_budget(payload_budget);
    // Cursor issue/expiry time is a correctness-relevant input. Pin it so
    // A/B equality cannot fail or pass by crossing a wall-clock second.
    let authority =
        CursorAuthorityV2::process_local()?.with_clock_for_tests(Arc::new(FixedCursorClock));
    dispatcher
        .cursor_authority
        .set(authority)
        .map_err(|_uninstalled_authority| "fixture cursor authority was already initialized")?;
    let correlation = RequestCorrelationV1::from_raw(77).ok_or("77 must be nonzero")?;
    let budget = RequestBudgetV1::unbounded().with_correlation(correlation);
    if cancelled {
        budget.cancel_handle().cancel();
    }
    let enabled = dispatcher.dispatch(request.clone(), &budget);
    let disabled = dispatcher
        .with_query_stage_observation(QueryStageObservationPolicy::Disabled)
        .dispatch(request, &budget);
    let mut enabled = serde_json::to_value(enabled)?;
    let disabled = serde_json::to_value(disabled)?;
    let object = enabled
        .as_object_mut()
        .ok_or("response is not an enum object")?;
    let is_error = object.get("kind").and_then(serde_json::Value::as_str) == Some("Error");
    let payload = object
        .get_mut("payload")
        .ok_or("missing response payload")?;
    if expected_uncut_rows > 1 {
        let returned = payload
            .get("results")
            .and_then(serde_json::Value::as_array)
            .ok_or("byte-capped query must succeed")?
            .len();
        if returned == 0
            || returned >= expected_uncut_rows
            || payload
                .get("next_cursor")
                .is_none_or(serde_json::Value::is_null)
        {
            return Err("byte-budget fixture did not exercise a nonempty continued prefix".into());
        }
    }
    if let Some(explanation) = payload.get_mut("explanation") {
        if explanation
            .get("request_id")
            .and_then(serde_json::Value::as_u64)
            != Some(77)
        {
            return Err("query stage observation lost request correlation".into());
        }
        let stages = explanation
            .get_mut("stage_timings")
            .ok_or("missing stage field")?;
        if stages.as_array().is_none_or(Vec::is_empty) {
            return Err("enabled query did not observe stages".into());
        }
        *stages = serde_json::Value::Null;
    } else if !is_error {
        return Err("expected a successful query or typed error".into());
    }
    if enabled != disabled {
        return Err(format!(
            "observation policy changed query outcome: enabled={enabled}, disabled={disabled}"
        )
        .into());
    }
    Ok(())
}

#[test]
fn lexical_stage_policy_preserves_query_outcome() -> TestResult {
    compare_policy(SearchPlaneQueryIpcRequest::Text(request_text()))
}

#[test]
fn lexical_stage_policy_preserves_small_response_budget_refusals() -> TestResult {
    for bytes in [1, 200, 1024, 2000, 3500] {
        compare_policy_with_budget(
            SearchPlaneQueryIpcRequest::Text(request_text()),
            ResponsePayloadBudget::new(bytes)?,
            false,
        )?;
    }
    Ok(())
}

#[test]
fn lexical_stage_policy_preserves_byte_capped_page_and_cursor() -> TestResult {
    let rows = (0..5)
        .map(|index| {
            let mut row = candidate(&format!("lex-{index}"), 1.0);
            row.snippet = "x".repeat(900);
            row
        })
        .collect::<Vec<_>>();
    compare_policy_with_rows(
        SearchPlaneQueryIpcRequest::Text(request_text()),
        ResponsePayloadBudget::new(3500)?,
        false,
        rows,
    )
}

#[test]
fn stage_policy_does_not_disable_cancellation() -> TestResult {
    compare_policy_with_budget(
        SearchPlaneQueryIpcRequest::Text(request_text()),
        ResponsePayloadBudget::DEFAULT,
        true,
    )
}

#[test]
fn semantic_stage_policy_preserves_query_outcome() -> TestResult {
    compare_policy(SearchPlaneQueryIpcRequest::Semantic(SemanticQueryRequest {
        query_text: "scope alpha".to_string(),
        constraints: QueryConstraintSetV1::unconstrained(),
        generation: Some(ready_pin()),
        generation_selector: None,
        lexical_scope: None,
        top_k: 10,
    }))
}

#[test]
fn hybrid_stage_policy_preserves_query_outcome() -> TestResult {
    compare_policy(SearchPlaneQueryIpcRequest::Hybrid(HybridQueryRequest {
        text_query: request_text(),
        semantic_query_text: "scope alpha".to_string(),
        generation: Some(ready_pin()),
        generation_selector: None,
        top_k: 10,
    }))
}
