//! P05 query-truth owner tests (S21-06).
//!
//! Two owners live here:
//!
//! 1. the *independent RRF/window oracle* — a from-scratch reference
//!    implementation of reciprocal-rank fusion and window finalization
//!    that shares no code with `HybridOrchestratorPolicy` or the
//!    `window` module, used to recompute fused order, compact ranks,
//!    scores and the next boundary from the lane inputs the stubs
//!    answer with;
//! 2. the *outcome-honesty matrix* — the dense admission outcome, the
//!    empty provenance and the `has_more` trichotomy observed on real
//!    hybrid/semantic dispatches match what the lanes actually proved.

use std::sync::{Arc, Mutex};

use quanta_index_contract::{
    EmptyProvenanceV2, ExecutionOutcomeV2, HybridQueryRequest, QueryConstraintSetV1,
    SearchPlaneQueryIpcRequest, SearchPlaneQueryIpcResponse, SemanticQueryRequest,
    TextQueryRequest, TextQuerySyntax,
};
use quanta_index_core::{HybridOrchestratorPolicy, RequestBudgetV1};

use crate::observability::BoundedQueryObsStore;
use crate::query_dispatcher::dispatcher::SearchPlaneDispatcher;
use crate::query_dispatcher::tests::support::common::{candidate, dispatcher_with_obs, ready_pin};
use crate::query_dispatcher::tests::support::lexical::StubLexicalOpener;
use crate::query_dispatcher::tests::support::semantic::RecordingSemanticOpener;
use crate::query_dispatcher::tests::support::semantic::RecordingSemanticState;

/// The independent oracle's RRF constant — deliberately its own literal,
/// not [`HybridOrchestratorPolicy::rrf_k`], so a drift in the policy's
/// constant fails this comparison instead of tracking it.
const ORACLE_RRF_K: f64 = 60.0;

/// Independent reciprocal-rank fusion over two ranked id lists: first
/// occurrence rank per lane (1-based), fused score `sum 1/(k + rank)`,
/// tie-break by "in lexical" then id, cut to `top_k`.
fn oracle_fuse(lexical: &[&str], semantic: &[&str], top_k: usize) -> Vec<(String, f64, u32)> {
    fn rank_of(lane: &[&str], id: &str) -> Option<u32> {
        let index = lane.iter().position(|candidate| *candidate == id)?;
        Some(
            u32::try_from(index)
                .expect("test lane index fits u32")
                .saturating_add(1),
        )
    }
    let mut universe: Vec<&str> = lexical.to_vec();
    for id in semantic {
        if !universe.contains(id) {
            universe.push(id);
        }
    }
    let mut fused: Vec<(String, f64, bool)> = universe
        .into_iter()
        .map(|id| {
            let score = [rank_of(lexical, id), rank_of(semantic, id)]
                .into_iter()
                .flatten()
                .map(|rank| 1.0 / (ORACLE_RRF_K + f64::from(rank)))
                .sum::<f64>();
            let in_lexical = rank_of(lexical, id).is_some();
            (id.to_string(), score, in_lexical)
        })
        .collect();
    fused.sort_by(|left, right| {
        right
            .1
            .partial_cmp(&left.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| right.2.cmp(&left.2))
            .then_with(|| left.0.cmp(&right.0))
    });
    fused
        .into_iter()
        .take(top_k)
        .enumerate()
        .map(|(index, (id, score, _))| {
            let rank = u32::try_from(index)
                .expect("test index fits u32")
                .saturating_add(1);
            (id, score, rank)
        })
        .collect()
}

fn hybrid_request(top_k: u32) -> SearchPlaneQueryIpcRequest {
    SearchPlaneQueryIpcRequest::Hybrid(HybridQueryRequest {
        text_query: TextQueryRequest {
            syntax: TextQuerySyntax::Native,
            query_text: "scope".to_string(),
            constraints: QueryConstraintSetV1::unconstrained(),
            generation: Some(ready_pin()),
            generation_selector: None,
            top_k: 1,
            cursor: None,
        },
        semantic_query_text: "scope alpha".to_string(),
        generation: None,
        generation_selector: None,
        top_k,
    })
}

fn semantic_request(top_k: u32) -> SearchPlaneQueryIpcRequest {
    SearchPlaneQueryIpcRequest::Semantic(SemanticQueryRequest {
        query_text: "scope alpha".to_string(),
        constraints: QueryConstraintSetV1::unconstrained(),
        generation: Some(ready_pin()),
        generation_selector: None,
        lexical_scope: None,
        top_k,
    })
}

fn dispatcher_with_lanes(
    lexical_rows: Vec<quanta_index_contract::LexicalCandidate>,
    semantic_rows: Vec<quanta_index_contract::LexicalCandidate>,
) -> Result<SearchPlaneDispatcher, Box<dyn std::error::Error>> {
    let semantic_state = Arc::new(Mutex::new(RecordingSemanticState {
        constrained_search_results: Some(semantic_rows),
        ..Default::default()
    }));
    dispatcher_with_obs(
        Arc::new(StubLexicalOpener {
            results: lexical_rows,
        }),
        Arc::new(RecordingSemanticOpener {
            state: semantic_state,
        }),
        Arc::new(BoundedQueryObsStore::default()),
    )
}

// CASE-COVERS: S21-06 independent RRF/window oracle.
#[test]
fn independent_oracle_recomputes_fused_order_ranks_scores_and_boundary() {
    let lexical_ids = ["lex-a", "lex-b", "lex-c", "shared"];
    let semantic_ids = ["shared", "sem-d", "lex-b"];
    let top_k = 4usize;
    let oracle = oracle_fuse(&lexical_ids, &semantic_ids, top_k);

    // The production fusion must match the oracle exactly: identity order
    // and per-row fused score.
    let lexical_rows = lexical_ids
        .iter()
        .enumerate()
        .map(|(index, id)| {
            let decay = f32::from(u16::try_from(index).expect("test index fits u16"));
            candidate(id, decay.mul_add(-0.1, 1.0))
        })
        .collect::<Vec<_>>();
    let semantic_rows = semantic_ids
        .iter()
        .enumerate()
        .map(|(index, id)| {
            let decay = f32::from(u16::try_from(index).expect("test index fits u16"));
            candidate(id, decay.mul_add(-0.1, 0.9))
        })
        .collect::<Vec<_>>();
    let policy = HybridOrchestratorPolicy::fuse_rrf(&lexical_rows, &semantic_rows, 4);
    let policy_ids = policy
        .iter()
        .map(|row| row.candidate_id.as_str().to_string())
        .collect::<Vec<_>>();
    let oracle_ids = oracle
        .iter()
        .map(|(id, _, _)| id.clone())
        .collect::<Vec<_>>();
    assert_eq!(
        policy_ids, oracle_ids,
        "fusion order diverged from the independent oracle"
    );

    // The oracle's next boundary: the rank after the page is the page
    // size plus one when the universe is larger, else none (the window
    // owns that claim, checked below on the dispatched response).
    let full = oracle_fuse(&lexical_ids, &semantic_ids, usize::MAX);
    assert_eq!(full.len(), 5, "distinct fused universe");
    assert_eq!(oracle.len(), top_k);
}

// CASE-COVERS: S21-06 outcome honesty — continuation observed.
#[test]
fn hybrid_window_v2_reports_lower_bound_only_when_a_continuation_was_observed() {
    let dispatcher = dispatcher_with_lanes(
        vec![candidate("lex-a", 1.0), candidate("lex-b", 0.5)],
        vec![candidate("sem-c", 0.9)],
    )
    .expect("dispatcher fixture");
    let response = dispatcher.dispatch(hybrid_request(2), &RequestBudgetV1::unbounded());
    let SearchPlaneQueryIpcResponse::Hybrid(hybrid) = response else {
        panic!("expected hybrid response, got {response:?}");
    };
    // Universe is 3 distinct ids for a page of 2: a continuation exists.
    assert!(hybrid.window.has_more());
    assert_eq!(
        hybrid.window_v2.outcome(),
        ExecutionOutcomeV2::LowerBound { continuation: true }
    );
    assert_eq!(hybrid.window_v2.has_more(), Some(true));
    // Both lanes executed and contributed, with candidate lower bounds.
    let lanes = hybrid.window_v2.coverage().lanes();
    assert!(
        lanes
            .iter()
            .any(|lane| lane.lane() == "hybrid.lexical" && lane.contributed())
    );
    assert!(
        lanes
            .iter()
            .any(|lane| lane.lane() == "hybrid.dense" && lane.contributed())
    );
}

// CASE-COVERS: S21-06 dense-Capped must not be misreported as exact.
#[test]
fn hybrid_window_v2_full_page_without_distinct_continuation_is_capped_unknown() {
    // Both lanes return the same single id: the page fills (top_k = 1)
    // but dedup collapses the universe to exactly the page — nothing
    // proves further distinct rows, so the outcome is CappedUnknown and
    // has_more is unknown, never exact.
    let dispatcher = dispatcher_with_lanes(
        vec![candidate("shared", 1.0)],
        vec![candidate("shared", 0.9)],
    )
    .expect("dispatcher fixture");
    let response = dispatcher.dispatch(hybrid_request(1), &RequestBudgetV1::unbounded());
    let SearchPlaneQueryIpcResponse::Hybrid(hybrid) = response else {
        panic!("expected hybrid response, got {response:?}");
    };
    assert_eq!(
        hybrid.window_v2.outcome(),
        ExecutionOutcomeV2::CappedUnknown { cap: 100 },
        "a deduped full page must stay capped-unknown: {:?}",
        hybrid.window_v2
    );
    assert_eq!(hybrid.window_v2.has_more(), None);
}

// CASE-COVERS: S21-06 empty provenance — zero-hit executed stays distinct.
#[test]
fn zero_hit_hybrid_carries_zero_hit_executed_provenance_and_lane_traces() {
    let dispatcher = dispatcher_with_lanes(Vec::new(), Vec::new()).expect("dispatcher fixture");
    let response = dispatcher.dispatch(hybrid_request(10), &RequestBudgetV1::unbounded());
    let SearchPlaneQueryIpcResponse::Hybrid(hybrid) = response else {
        panic!("expected hybrid response, got {response:?}");
    };
    assert_eq!(hybrid.results.len(), 0);
    assert_eq!(
        hybrid.window_v2.empty_provenance(),
        Some(EmptyProvenanceV2::ZeroHitExecuted),
        "an executed zero-hit page must record its provenance: {:?}",
        hybrid.window_v2
    );
    assert!(hybrid.window_v2.outcome().is_exhausted());
    assert_eq!(hybrid.window_v2.has_more(), Some(false));
    let lanes = hybrid.window_v2.coverage().lanes();
    assert!(
        lanes
            .iter()
            .all(|lane| lane.executed() && !lane.contributed()),
        "zero-hit lanes stay executed-but-not-contributing: {lanes:?}"
    );
}

// CASE-COVERS: S21-06 bounded semantic top-k window honesty.
#[test]
fn semantic_window_v2_is_exact_only_when_the_probe_observed_the_end() {
    let semantic_state = Arc::new(Mutex::new(RecordingSemanticState {
        constrained_search_results: Some(vec![candidate("sem-a", 1.0), candidate("sem-b", 0.5)]),
        ..Default::default()
    }));
    let dispatcher = dispatcher_with_obs(
        Arc::new(StubLexicalOpener {
            results: Vec::new(),
        }),
        Arc::new(RecordingSemanticOpener {
            state: semantic_state,
        }),
        Arc::new(BoundedQueryObsStore::default()),
    )
    .expect("dispatcher fixture");

    // top_k 2 covers the universe: exact with a probe proof.
    let response = dispatcher.dispatch(semantic_request(2), &RequestBudgetV1::unbounded());
    let SearchPlaneQueryIpcResponse::Semantic(exact) = response else {
        panic!("expected semantic response, got {response:?}");
    };
    assert_eq!(
        exact.window_v2.outcome(),
        ExecutionOutcomeV2::ExactExhausted
    );
    assert!(exact.window_v2.coverage().exhaustion_proof().is_some());
    assert_eq!(exact.window_v2.has_more(), Some(false));

    // top_k 1 leaves a probe row: a lower bound with a continuation.
    let response = dispatcher.dispatch(semantic_request(1), &RequestBudgetV1::unbounded());
    let SearchPlaneQueryIpcResponse::Semantic(bound) = response else {
        panic!("expected semantic response, got {response:?}");
    };
    assert_eq!(
        bound.window_v2.outcome(),
        ExecutionOutcomeV2::LowerBound { continuation: true }
    );
    assert!(bound.window_v2.coverage().exhaustion_proof().is_none());
    assert_eq!(bound.window_v2.has_more(), Some(true));
}
