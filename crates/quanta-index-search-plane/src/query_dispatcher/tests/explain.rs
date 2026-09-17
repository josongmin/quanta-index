//! Explain-route unit tests (QI-BB-022): a hybrid row is reconciled lane
//! by lane against the plan and against its own RRF arithmetic.
//!
//! The lexical double scores every indexed row at exactly its carried
//! score, so the oracle for the lexical lane is the row the double was
//! given; the oracle for the fused score is an RRF sum recomputed here.

use std::sync::{Arc, Mutex};

use quanta_index_contract::{
    ExplainCandidateV1, HybridCandidateV1, HybridLaneContributionV1, HybridLaneV1, PlannerStage,
    QueryConstraintSetV1, SearchExplanation, SearchPlaneExplainQueryRequest,
    SearchPlaneQueryIpcRequest, SearchPlaneQueryIpcResponse, TextQueryRequest, TextQuerySyntax,
};
use quanta_index_core::RequestBudgetV1;

use crate::query_dispatcher::dispatcher::SearchPlaneDispatcher;
use crate::query_dispatcher::errors::ERR_INVALID;
use crate::query_dispatcher::tests::support::common::{
    TestResult, candidate, ipc_error_from, ready_ledger, ready_pin, test_activation_catalog,
};
use crate::query_dispatcher::tests::support::lexical::StubLexicalOpener;
use crate::query_dispatcher::tests::support::repo_map::StubRepoMapQueryPort;
use crate::query_dispatcher::tests::support::semantic::{
    RecordingSemanticOpener, RecordingSemanticState,
};
use crate::query_dispatcher::tests::support::structural::FailClosedStructuralProducer;

type Outcome<T> = Result<T, Box<dyn std::error::Error>>;

/// A dispatcher whose lexical index holds exactly `rows`, each explaining
/// to its own score under a unit boost.
fn dispatcher_over(
    rows: Vec<quanta_index_contract::LexicalCandidate>,
) -> Outcome<SearchPlaneDispatcher> {
    Ok(SearchPlaneDispatcher::new(
        Arc::new(StubLexicalOpener { results: rows }),
        Arc::new(RecordingSemanticOpener {
            state: Arc::new(Mutex::new(RecordingSemanticState::default())),
        }),
        Arc::new(StubRepoMapQueryPort),
        Arc::new(FailClosedStructuralProducer),
        ready_ledger(),
        test_activation_catalog()?,
    ))
}

fn query() -> TextQueryRequest {
    TextQueryRequest {
        syntax: TextQuerySyntax::Native,
        query_text: "scope".to_string(),
        constraints: QueryConstraintSetV1::unconstrained(),
        generation: None,
        generation_selector: None,
        top_k: 1,
    }
}

/// The RRF sum the plane carries for these ranks, under its k = 60.
fn rrf(ranks: &[u32]) -> f64 {
    ranks
        .iter()
        .map(|rank| 1.0 / (60.0 + f64::from(*rank)))
        .sum()
}

fn both_lanes(id: &str, lexical_raw: f32, dense_raw: f32) -> HybridCandidateV1 {
    HybridCandidateV1 {
        candidate: candidate(id, lexical_raw),
        fused_score: rrf(&[1, 2]),
        contributions: vec![
            HybridLaneContributionV1 {
                lane: HybridLaneV1::Lexical,
                rank: 1,
                raw_score: lexical_raw,
            },
            HybridLaneContributionV1 {
                lane: HybridLaneV1::Dense,
                rank: 2,
                raw_score: dense_raw,
            },
        ],
    }
}

fn explain(
    dispatcher: &SearchPlaneDispatcher,
    candidate: ExplainCandidateV1,
    text_query: Option<TextQueryRequest>,
) -> SearchPlaneQueryIpcResponse {
    dispatcher.dispatch(
        SearchPlaneQueryIpcRequest::Explain(SearchPlaneExplainQueryRequest {
            generation: ready_pin(),
            candidate,
            text_query,
        }),
        &RequestBudgetV1::unbounded(),
    )
}

fn explanation_of(response: SearchPlaneQueryIpcResponse) -> Outcome<SearchExplanation> {
    let SearchPlaneQueryIpcResponse::Explain(explain) = response else {
        return Err(format!("expected Explain response, got {response:?}").into());
    };
    Ok(explain.explanation)
}

fn trace_says(explanation: &SearchExplanation, detail: &str) -> bool {
    explanation
        .planner_trace
        .iter()
        .any(|entry| entry.stage == PlannerStage::Merge && entry.detail == detail)
}

fn row_names(explanation: &SearchExplanation) -> Vec<&str> {
    explanation
        .contributions
        .iter()
        .map(|row| row.signal_name.as_ref())
        .collect()
}

// CASE-COVERS: a genuine both-lane row reconciles on both counts, with one
// row per lane and the fused row equal to the recomputed RRF.
#[test]
fn a_both_lane_hybrid_row_reconciles_its_lexical_lane_and_its_rrf_score() -> TestResult {
    let dispatcher = dispatcher_over(vec![candidate("alpha", 1.25)])?;
    let hybrid = both_lanes("alpha", 1.25, 0.5);
    let explanation = explanation_of(explain(
        &dispatcher,
        ExplainCandidateV1::Hybrid(hybrid.clone()),
        Some(query()),
    ))?;
    if explanation.strategy != "hybrid_score_trace"
        || !trace_says(&explanation, "explain.candidate_indexed=true")
        || !trace_says(&explanation, "explain.candidate_matched=true")
        || !trace_says(&explanation, "explain.score_reconciled=true")
        || !trace_says(&explanation, "explain.fused_reconciled=true")
        || !trace_says(&explanation, "explain.rrf_k=60; ranks=lexical#1,dense#2")
    {
        return Err(format!("both-lane trace: {explanation:?}").into());
    }
    if row_names(&explanation) != ["lexical.bm25", "dense.cosine", "hybrid.rrf"] {
        return Err(format!("both-lane rows: {:?}", explanation.contributions).into());
    }
    let [lexical, dense, fused] = explanation.contributions.as_slice() else {
        return Err("three rows".into());
    };
    // The fused row is the f64 RRF score narrowed to the row's f32, so it is
    // within an f32 rounding step (about 4e-9 at this magnitude) of it.
    if lexical.contribution.to_bits() != 1.25_f32.to_bits()
        || dense.contribution.to_bits() != 0.5_f32.to_bits()
        || (f64::from(fused.contribution) - hybrid.fused_score).abs() > 1e-8
    {
        return Err(format!("row values: {:?}", explanation.contributions).into());
    }
    if explanation.ranker_weights_hash == [0u8; 32] {
        return Err("a hybrid trace pins its ranker weights".into());
    }
    Ok(())
}

// CASE-COVERS: a row whose carried lexical raw score is not what the plan
// emits, or whose fused score is not the RRF of its ranks, is not
// reconciled — each defect is reported on its own axis.
#[test]
fn a_hybrid_row_with_stale_provenance_is_not_reconciled_on_that_axis() -> TestResult {
    let dispatcher = dispatcher_over(vec![candidate("alpha", 1.25)])?;
    // The lexical lane's carried raw score was scored under another plan.
    let stale_lexical = both_lanes("alpha", 1.0, 0.5);
    let explanation = explanation_of(explain(
        &dispatcher,
        ExplainCandidateV1::Hybrid(stale_lexical),
        Some(query()),
    ))?;
    if !trace_says(&explanation, "explain.score_reconciled=false")
        || !trace_says(&explanation, "explain.fused_reconciled=true")
    {
        return Err(format!("stale lexical lane: {explanation:?}").into());
    }
    // The fused score is not the RRF of the carried ranks.
    let mut stale_fusion = both_lanes("alpha", 1.25, 0.5);
    stale_fusion.fused_score = rrf(&[1, 3]);
    let explanation = explanation_of(explain(
        &dispatcher,
        ExplainCandidateV1::Hybrid(stale_fusion),
        Some(query()),
    ))?;
    if !trace_says(&explanation, "explain.score_reconciled=true")
        || !trace_says(&explanation, "explain.fused_reconciled=false")
    {
        return Err(format!("stale fusion: {explanation:?}").into());
    }
    Ok(())
}

// CASE-COVERS: a dense-only row is reconciled when the plan does not match
// it (the lexical lane rightly did not see it), and is not when the plan
// does match it (the lexical lane should have seen it); the dense row is
// carried either way and the fused row is the single-lane RRF.
#[test]
fn a_dense_only_hybrid_row_is_reconciled_only_when_the_plan_does_not_match_it() -> TestResult {
    let dense_only = HybridCandidateV1 {
        candidate: candidate("delta", 0.75),
        fused_score: rrf(&[1]),
        contributions: vec![HybridLaneContributionV1 {
            lane: HybridLaneV1::Dense,
            rank: 1,
            raw_score: 0.75,
        }],
    };
    // The double indexes `delta`, but it was not the lexical lane's row:
    // the plan matching it contradicts the carried provenance.
    let matching = dispatcher_over(vec![candidate("delta", 2.0)])?;
    let explanation = explanation_of(explain(
        &matching,
        ExplainCandidateV1::Hybrid(dense_only.clone()),
        Some(query()),
    ))?;
    if !trace_says(&explanation, "explain.candidate_matched=true")
        || !trace_says(&explanation, "explain.score_reconciled=false")
        || row_names(&explanation) != ["lexical.bm25", "dense.cosine", "hybrid.rrf"]
    {
        return Err(format!("dense-only row the plan matches: {explanation:?}").into());
    }
    // The double does not index `delta` at all: consistent with a row only
    // the dense lane saw.
    let unmatching = dispatcher_over(vec![candidate("alpha", 2.0)])?;
    let explanation = explanation_of(explain(
        &unmatching,
        ExplainCandidateV1::Hybrid(dense_only),
        Some(query()),
    ))?;
    if !trace_says(&explanation, "explain.candidate_indexed=false")
        || !trace_says(&explanation, "explain.score_reconciled=true")
        || !trace_says(&explanation, "explain.fused_reconciled=true")
        || row_names(&explanation) != ["dense.cosine", "hybrid.rrf"]
    {
        return Err(format!("dense-only row the plan does not match: {explanation:?}").into());
    }
    Ok(())
}

// CASE-COVERS: a hybrid row explains only under its query; a presence-only
// explain of one is a typed refusal, not a silent presence lookup.
#[test]
fn a_hybrid_row_without_its_query_is_refused() -> TestResult {
    let dispatcher = dispatcher_over(vec![candidate("alpha", 1.25)])?;
    let response = explain(
        &dispatcher,
        ExplainCandidateV1::Hybrid(both_lanes("alpha", 1.25, 0.5)),
        None,
    );
    let (code, message) = ipc_error_from(response).map_err(Box::<dyn std::error::Error>::from)?;
    if code != ERR_INVALID || !message.contains("text_query is required") {
        return Err(format!("unexpected refusal: {code} {message}").into());
    }
    Ok(())
}

// CASE-COVERS: the lexical explain is unchanged by the hybrid one — a
// lexical row still traces to one reconciled row under the lexical strategy.
#[test]
fn a_lexical_row_still_explains_as_a_lexical_score_trace() -> TestResult {
    let dispatcher = dispatcher_over(vec![candidate("alpha", 1.25)])?;
    let explanation = explanation_of(explain(
        &dispatcher,
        ExplainCandidateV1::Lexical(candidate("alpha", 1.25)),
        Some(query()),
    ))?;
    if explanation.strategy != "lexical_score_trace"
        || !trace_says(&explanation, "explain.score_reconciled=true")
        || row_names(&explanation) != ["lexical.bm25"]
        || explanation
            .planner_trace
            .iter()
            .any(|entry| entry.detail.starts_with("explain.fused_reconciled"))
    {
        return Err(format!("lexical trace: {explanation:?}").into());
    }
    Ok(())
}
