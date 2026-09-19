//! Explain-route unit tests (QI-BB-022): a hybrid row is re-derived lane
//! by lane against the index, never against its own payload.
//!
//! The lexical double scores every indexed row at exactly its carried
//! score, so the oracle for the lexical lane is the row the double was
//! given; the dense double stores one cosine per id and answers the rows
//! it is configured with, so the oracle for the dense lane is that cosine
//! and the oracle for the fusion is an RRF sum over the ranks the doubles
//! would produce, recomputed here. A payload that is internally consistent
//! but disagrees with the index fails on the axis it forged.

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

/// The dense query every hybrid explain here names.
const DENSE_QUERY: &str = "scope";

/// A dispatcher whose lexical index holds exactly `lexical_rows`, each
/// explaining to its own score under a unit boost, and whose dense lane
/// answers `dense_rows` and stores one cosine per `(id, cosine)`.
fn dispatcher_over(
    lexical_rows: Vec<quanta_index_contract::LexicalCandidate>,
    dense_rows: Vec<quanta_index_contract::LexicalCandidate>,
    stored_cosines: &[(&str, f32)],
) -> Outcome<(SearchPlaneDispatcher, Arc<Mutex<RecordingSemanticState>>)> {
    let state = Arc::new(Mutex::new(RecordingSemanticState {
        constrained_search_results: Some(dense_rows),
        stored_cosines: stored_cosines
            .iter()
            .map(|(id, cosine)| ((*id).to_string(), *cosine))
            .collect(),
        ..RecordingSemanticState::default()
    }));
    Ok((
        SearchPlaneDispatcher::new(
            Arc::new(StubLexicalOpener {
                results: lexical_rows,
            }),
            Arc::new(RecordingSemanticOpener {
                state: Arc::clone(&state),
            }),
            Arc::new(StubRepoMapQueryPort),
            Arc::new(FailClosedStructuralProducer),
            ready_ledger(),
            test_activation_catalog()?,
        ),
        state,
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

fn both_lanes(
    id: &str,
    lexical: (u32, f32),
    dense: (u32, f32),
    fused_score: f64,
) -> HybridCandidateV1 {
    HybridCandidateV1 {
        candidate: candidate(id, lexical.1),
        fused_score,
        contributions: vec![
            HybridLaneContributionV1 {
                lane: HybridLaneV1::Lexical,
                rank: lexical.0,
                raw_score: lexical.1,
            },
            HybridLaneContributionV1 {
                lane: HybridLaneV1::Dense,
                rank: dense.0,
                raw_score: dense.1,
            },
        ],
    }
}

fn dense_only(id: &str, rank: u32, raw_score: f32) -> HybridCandidateV1 {
    HybridCandidateV1 {
        candidate: candidate(id, raw_score),
        fused_score: rrf(&[rank]),
        contributions: vec![HybridLaneContributionV1 {
            lane: HybridLaneV1::Dense,
            rank,
            raw_score,
        }],
    }
}

fn explain(
    dispatcher: &SearchPlaneDispatcher,
    candidate: ExplainCandidateV1,
    text_query: Option<TextQueryRequest>,
    semantic_query_text: Option<&str>,
) -> SearchPlaneQueryIpcResponse {
    dispatcher.dispatch(
        SearchPlaneQueryIpcRequest::Explain(SearchPlaneExplainQueryRequest {
            generation: ready_pin(),
            candidate,
            text_query,
            semantic_query_text: semantic_query_text.map(str::to_string),
        }),
        &RequestBudgetV1::unbounded(),
    )
}

fn explain_hybrid(
    dispatcher: &SearchPlaneDispatcher,
    hybrid: HybridCandidateV1,
) -> Outcome<SearchExplanation> {
    explanation_of(explain(
        dispatcher,
        ExplainCandidateV1::Hybrid(hybrid),
        Some(query()),
        Some(DENSE_QUERY),
    ))
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

/// The sum of the `hybrid.rrf.*` rows: the fused score by the composition
/// rule on `ExplanationRow`.
fn rrf_row_sum(explanation: &SearchExplanation) -> f64 {
    explanation
        .contributions
        .iter()
        .filter(|row| row.signal_name.starts_with("hybrid.rrf."))
        .map(|row| f64::from(row.contribution))
        .sum()
}

/// The three reconciliation axes as the trace reports them.
fn axes(explanation: &SearchExplanation) -> (bool, bool, bool) {
    (
        trace_says(explanation, "explain.score_reconciled=true"),
        trace_says(explanation, "explain.dense_reconciled=true"),
        trace_says(explanation, "explain.fused_reconciled=true"),
    )
}

/// The index every both-lane case runs against: `alpha` is the lexical
/// lane's only row (rank 1, score 1.25); the dense lane ranks `beta` then
/// `alpha` (rank 2) and stores cosines 0.9 and 0.5 for them.
fn both_lane_index() -> Outcome<(SearchPlaneDispatcher, Arc<Mutex<RecordingSemanticState>>)> {
    dispatcher_over(
        vec![candidate("alpha", 1.25)],
        vec![candidate("beta", 0.9), candidate("alpha", 0.5)],
        &[("alpha", 0.5), ("beta", 0.9)],
    )
}

// CASE-COVERS: a genuine both-lane row reconciles on every axis against
// the index, with one lane-score row per lane, one RRF row per lane, and
// the RRF rows summing to the re-derived fused score.
#[test]
fn a_both_lane_hybrid_row_reconciles_every_axis_against_the_index() -> TestResult {
    let (dispatcher, state) = both_lane_index()?;
    let hybrid = both_lanes("alpha", (1, 1.25), (2, 0.5), rrf(&[1, 2]));
    let explanation = explain_hybrid(&dispatcher, hybrid.clone())?;
    if explanation.strategy != "hybrid_score_trace"
        || !trace_says(&explanation, "explain.candidate_indexed=true")
        || !trace_says(&explanation, "explain.candidate_matched=true")
        || axes(&explanation) != (true, true, true)
        || !trace_says(
            &explanation,
            "explain.rrf_k=60; carried_ranks=lexical#1,dense#2; rederived_ranks=lexical#1,dense#2",
        )
        || !trace_says(
            &explanation,
            "explain.dense_lane=rederived; cosine=0.500000",
        )
        || !trace_says(&explanation, "explain.fused_page_position=1")
    {
        return Err(format!("both-lane trace: {explanation:?}").into());
    }
    if row_names(&explanation)
        != [
            "lexical.bm25",
            "dense.cosine",
            "hybrid.rrf.lexical",
            "hybrid.rrf.dense",
        ]
    {
        return Err(format!("both-lane rows: {:?}", explanation.contributions).into());
    }
    let [lexical, dense, rrf_lexical, rrf_dense] = explanation.contributions.as_slice() else {
        return Err("four rows".into());
    };
    // Each RRF row is the f64 term narrowed to f32, so the sum is within an
    // f32 rounding step (about 4e-9 at this magnitude) of the fused score.
    if lexical.contribution.to_bits() != 1.25_f32.to_bits()
        || dense.contribution.to_bits() != 0.5_f32.to_bits()
        || (f64::from(rrf_lexical.contribution) - rrf(&[1])).abs() > 1e-8
        || (f64::from(rrf_dense.contribution) - rrf(&[2])).abs() > 1e-8
        || (rrf_row_sum(&explanation) - hybrid.fused_score).abs() > 1e-8
    {
        return Err(format!("row values: {:?}", explanation.contributions).into());
    }
    if explanation.ranker_weights_hash == [0u8; 32] {
        return Err("a hybrid trace pins its ranker weights".into());
    }
    // The dense lane was consulted for exactly this candidate, with the
    // embedded dense query, and both lanes were re-run once.
    let (scored, searched) = {
        let state = state
            .lock()
            .map_err(|err| format!("semantic state poisoned: {err}"))?;
        (state.scored_candidates.clone(), state.search_vectors.len())
    };
    if scored.len() != 1
        || scored
            .first()
            .is_none_or(|(id, vector)| id != "alpha" || vector.is_empty())
        || searched != 1
    {
        return Err(format!(
            "the explain consults the index once per lane: scored={scored:?} searched={searched}"
        )
        .into());
    }
    Ok(())
}

// CASE-COVERS: a payload that is internally consistent — its fused score is
// exactly the RRF of the ranks it carries — but whose ranks are not what
// the index produces is not reconciled on the fusion axis. Before the
// re-derivation this payload reconciled, since the RRF was recomputed from
// the payload's own ranks.
#[test]
fn a_self_consistent_forged_rank_is_not_reconciled_on_the_fusion_axis() -> TestResult {
    let (dispatcher, _state) = both_lane_index()?;
    // The dense lane really ranks alpha second; the payload claims first,
    // and carries the fused score that claim implies.
    let forged = both_lanes("alpha", (1, 1.25), (1, 0.5), rrf(&[1, 1]));
    let explanation = explain_hybrid(&dispatcher, forged)?;
    if axes(&explanation) != (true, true, false)
        || !trace_says(
            &explanation,
            "explain.rrf_k=60; carried_ranks=lexical#1,dense#1; rederived_ranks=lexical#1,dense#2",
        )
        || (rrf_row_sum(&explanation) - rrf(&[1, 2])).abs() > 1e-8
    {
        return Err(format!("forged rank: {explanation:?}").into());
    }
    // A forged fused score over genuine ranks fails the same axis.
    let forged_score = both_lanes("alpha", (1, 1.25), (2, 0.5), rrf(&[1, 3]));
    let explanation = explain_hybrid(&dispatcher, forged_score)?;
    if axes(&explanation) != (true, true, false) {
        return Err(format!("forged fused score: {explanation:?}").into());
    }
    Ok(())
}

// CASE-COVERS: a carried dense raw score that is not the stored vector's
// cosine is not reconciled on the dense axis, and only there. Before the
// re-derivation the dense row was echoed from the payload.
#[test]
fn a_forged_dense_score_is_not_reconciled_on_the_dense_axis() -> TestResult {
    let (dispatcher, _state) = both_lane_index()?;
    let forged = both_lanes("alpha", (1, 1.25), (2, 0.95), rrf(&[1, 2]));
    let explanation = explain_hybrid(&dispatcher, forged)?;
    if axes(&explanation) != (true, false, true) {
        return Err(format!("forged dense score: {explanation:?}").into());
    }
    let Some(dense) = explanation
        .contributions
        .iter()
        .find(|row| row.signal_name.as_ref() == "dense.cosine")
    else {
        return Err("the dense row is the re-derived cosine".into());
    };
    if dense.contribution.to_bits() != 0.5_f32.to_bits() {
        return Err(format!("the dense row is re-derived, not echoed: {dense:?}").into());
    }
    Ok(())
}

// CASE-COVERS: a carried lexical raw score that is not what the plan emits
// is not reconciled on the lexical axis, and only there.
#[test]
fn a_forged_lexical_score_is_not_reconciled_on_the_lexical_axis() -> TestResult {
    let (dispatcher, _state) = both_lane_index()?;
    let forged = both_lanes("alpha", (1, 1.0), (2, 0.5), rrf(&[1, 2]));
    let explanation = explain_hybrid(&dispatcher, forged)?;
    if axes(&explanation) != (false, true, true) {
        return Err(format!("forged lexical score: {explanation:?}").into());
    }
    Ok(())
}

// CASE-COVERS: a dense-only row reconciles when the plan does not match it
// and the dense lane ranks it where the row says; the rows are the dense
// cosine and the single-lane RRF term. The same row is not reconciled on
// the lexical axis when the plan does match it (the lexical lane should
// have seen it), and not on the dense axis when the generation stores no
// vector for it.
#[test]
fn a_dense_only_hybrid_row_reconciles_only_when_the_index_agrees() -> TestResult {
    let row = dense_only("delta", 1, 0.75);
    // The lexical double does not index `delta`; the dense lane ranks it
    // first and stores its cosine.
    let (consistent, _) = dispatcher_over(
        vec![candidate("alpha", 2.0)],
        vec![candidate("delta", 0.75)],
        &[("delta", 0.75)],
    )?;
    let explanation = explain_hybrid(&consistent, row.clone())?;
    if !trace_says(&explanation, "explain.candidate_indexed=false")
        || axes(&explanation) != (true, true, true)
        || row_names(&explanation) != ["dense.cosine", "hybrid.rrf.dense"]
        || (rrf_row_sum(&explanation) - rrf(&[1])).abs() > 1e-8
    {
        return Err(format!("dense-only row the index agrees with: {explanation:?}").into());
    }
    // The lexical double indexes `delta` and the plan matches it: the row
    // should have carried a lexical contribution.
    let (matching, _) = dispatcher_over(
        vec![candidate("delta", 2.0)],
        vec![candidate("delta", 0.75)],
        &[("delta", 0.75)],
    )?;
    let explanation = explain_hybrid(&matching, row.clone())?;
    if !trace_says(&explanation, "explain.candidate_matched=true")
        || axes(&explanation) != (false, true, false)
        || row_names(&explanation)
            != [
                "lexical.bm25",
                "dense.cosine",
                "hybrid.rrf.lexical",
                "hybrid.rrf.dense",
            ]
    {
        return Err(format!("dense-only row the plan matches: {explanation:?}").into());
    }
    // The generation stores no vector for `delta`: a carried dense score
    // has nothing to be the cosine of.
    let (unstored, _) = dispatcher_over(vec![candidate("alpha", 2.0)], Vec::new(), &[])?;
    let explanation = explain_hybrid(&unstored, row)?;
    if !trace_says(&explanation, "explain.dense_lane=absent")
        || axes(&explanation) != (true, false, false)
        || !row_names(&explanation).is_empty()
        || !trace_says(&explanation, "explain.fused_page_position=beyond_top_k")
    {
        return Err(format!("dense-only row with no stored vector: {explanation:?}").into());
    }
    Ok(())
}

// CASE-COVERS: a hybrid row explains only under both its queries; a
// presence-only explain of one, or one without its dense query, is a typed
// refusal, not a silent partial trace.
#[test]
fn a_hybrid_row_without_either_query_is_refused() -> TestResult {
    let (dispatcher, _state) = both_lane_index()?;
    let hybrid = both_lanes("alpha", (1, 1.25), (2, 0.5), rrf(&[1, 2]));
    for (text_query, semantic_query_text, fragment) in [
        (None, None, "text_query is required"),
        (Some(query()), None, "semantic_query_text is required"),
    ] {
        let response = explain(
            &dispatcher,
            ExplainCandidateV1::Hybrid(hybrid.clone()),
            text_query,
            semantic_query_text,
        );
        let (code, message) =
            ipc_error_from(response).map_err(Box::<dyn std::error::Error>::from)?;
        if code != ERR_INVALID || !message.contains(fragment) {
            return Err(format!("unexpected refusal: {code} {message}").into());
        }
    }
    Ok(())
}

// CASE-COVERS: the lexical explain is unchanged by the hybrid one — a
// lexical row still traces to one reconciled row under the lexical
// strategy, and a dense query beside it is refused.
#[test]
fn a_lexical_row_still_explains_as_a_lexical_score_trace() -> TestResult {
    let (dispatcher, state) = both_lane_index()?;
    let explanation = explanation_of(explain(
        &dispatcher,
        ExplainCandidateV1::Lexical(candidate("alpha", 1.25)),
        Some(query()),
        None,
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
    if !state
        .lock()
        .map_err(|err| format!("semantic state poisoned: {err}"))?
        .scored_candidates
        .is_empty()
    {
        return Err("a lexical explain never consults the dense lane".into());
    }
    let response = explain(
        &dispatcher,
        ExplainCandidateV1::Lexical(candidate("alpha", 1.25)),
        Some(query()),
        Some(DENSE_QUERY),
    );
    let (code, message) = ipc_error_from(response).map_err(Box::<dyn std::error::Error>::from)?;
    if code != ERR_INVALID || !message.contains("semantic_query_text must be absent") {
        return Err(format!("unexpected refusal: {code} {message}").into());
    }
    Ok(())
}

// CASE-COVERS: the re-run dense lane is the hybrid route's dense lane — it
// admits every dense candidate through the plan's exact filters (QI-BB-018
// 보완 #3) — so a dense-only row the filter excludes is not what the dense
// lane would have carried, and the explain says so on the fusion axis while
// the filter-only admission plan was asked about it.
#[test]
fn the_rerun_dense_lane_admits_candidates_through_the_same_filter_plan() -> TestResult {
    use std::collections::BTreeSet;

    use crate::query_dispatcher::tests::support::lexical::{
        RecordingLexicalOpener, RecordingLexicalState,
    };

    // The filter-only plan admits `beta` only; `delta` is stored and ranks
    // first in the dense lane but the plan excludes it.
    let lexical_state = Arc::new(Mutex::new(RecordingLexicalState {
        admitted_ids: Some(BTreeSet::from(["beta".to_string()])),
        ..RecordingLexicalState::default()
    }));
    let semantic_state = Arc::new(Mutex::new(RecordingSemanticState {
        constrained_search_results: Some(vec![candidate("delta", 0.9), candidate("beta", 0.6)]),
        stored_cosines: [("delta".to_string(), 0.9), ("beta".to_string(), 0.6)]
            .into_iter()
            .collect(),
        ..RecordingSemanticState::default()
    }));
    let dispatcher = SearchPlaneDispatcher::new(
        Arc::new(RecordingLexicalOpener {
            state: Arc::clone(&lexical_state),
            results: Vec::new(),
        }),
        Arc::new(RecordingSemanticOpener {
            state: Arc::clone(&semantic_state),
        }),
        Arc::new(StubRepoMapQueryPort),
        Arc::new(FailClosedStructuralProducer),
        ready_ledger(),
        test_activation_catalog()?,
    );
    let filtered_query = TextQueryRequest {
        query_text: "file:src/lib.rs scope".to_string(),
        ..query()
    };
    // The carried row claims the dense lane ranked `delta` first at its
    // stored cosine: true of an unfiltered lane, not of the filtered one.
    let excluded = dense_only("delta", 1, 0.9);
    let explanation = explanation_of(explain(
        &dispatcher,
        ExplainCandidateV1::Hybrid(excluded),
        Some(filtered_query.clone()),
        Some(DENSE_QUERY),
    ))?;
    if axes(&explanation) != (true, true, false)
        || !trace_says(
            &explanation,
            "explain.dense_lane=rederived; cosine=0.900000",
        )
        || !trace_says(
            &explanation,
            "explain.rrf_k=60; carried_ranks=dense#1; rederived_ranks=lexical#absent,dense#absent",
        )
        || !explanation
            .planner_trace
            .iter()
            .any(|entry| entry.detail == "hybrid.filters=exact:file")
    {
        return Err(format!("an excluded dense-only row: {explanation:?}").into());
    }
    // The admitted row reconciles: the filtered dense lane ranks it first.
    let admitted = dense_only("beta", 1, 0.6);
    let explanation = explanation_of(explain(
        &dispatcher,
        ExplainCandidateV1::Hybrid(admitted),
        Some(filtered_query),
        Some(DENSE_QUERY),
    ))?;
    if axes(&explanation) != (true, true, true)
        || !trace_says(
            &explanation,
            "explain.rrf_k=60; carried_ranks=dense#1; rederived_ranks=lexical#absent,dense#1",
        )
    {
        return Err(format!("an admitted dense-only row: {explanation:?}").into());
    }
    let admission_calls = lexical_state
        .lock()
        .map_err(|err| format!("lexical state poisoned: {err}"))?
        .admission_calls
        .len();
    if admission_calls < 2 {
        return Err(format!(
            "each explain re-runs the dense lane through admission; {admission_calls} calls"
        )
        .into());
    }
    Ok(())
}
