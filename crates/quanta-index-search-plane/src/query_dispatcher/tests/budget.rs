use std::sync::{Arc, Mutex};

use quanta_index_contract::{
    HybridQueryRequest, HybridSeedQueryRequest, SearchPlaneQueryIpcRequest, SemanticCorpusKindV1,
    SemanticSeedCorpusBudgetV1, SymbolQueryRequest, TextQueryRequest, TextQuerySyntax,
};
use quanta_index_core::{REQUEST_CANCELLED_CODE, RequestBudgetV1};

use crate::observability::BoundedQueryObsStore;
use crate::query_dispatcher::tests::support::common::{
    TestResult, candidate, dispatcher_with_obs, ipc_error_from, ready_pin,
};
use crate::query_dispatcher::tests::support::history::history_query_request;
use crate::query_dispatcher::tests::support::lexical::{
    RecordingLexicalOpener, RecordingLexicalState, RejectLexicalOpener,
};
use crate::query_dispatcher::tests::support::repo_map::repo_map_request;
use crate::query_dispatcher::tests::support::runtime_metadata::runtime_query_request;
use crate::query_dispatcher::tests::support::semantic::{
    RecordingSemanticOpener, RecordingSemanticState, RejectSemanticOpener, semantic_focus_request,
};

/// Every route observes its budget before it opens anything.
///
/// An already-interrupted request is answered with the typed code naming
/// the entry checkpoint, no opener is consulted (the reject openers would
/// turn a consulted open into `NOT_IMPLEMENTED`), and the interruption
/// lands in its own error counter rather than `other`.
#[test]
fn every_route_refuses_an_interrupted_budget_at_entry_without_opening() -> TestResult {
    let obs_sink = Arc::new(BoundedQueryObsStore::default());
    let dispatcher = dispatcher_with_obs(
        Arc::new(RejectLexicalOpener),
        Arc::new(RejectSemanticOpener),
        obs_sink.clone(),
    )?;
    let text = || TextQueryRequest {
        syntax: TextQuerySyntax::Native,
        query_text: "needle".to_string(),
        constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
        generation: Some(ready_pin()),
        generation_selector: None,
        top_k: 10,
    };
    let routes: Vec<(&str, SearchPlaneQueryIpcRequest)> = vec![
        ("lexical:entry", SearchPlaneQueryIpcRequest::Text(text())),
        (
            "symbol:entry",
            SearchPlaneQueryIpcRequest::Symbol(SymbolQueryRequest {
                syntax: TextQuerySyntax::Native,
                query_text: "needle".to_string(),
                constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                generation: Some(ready_pin()),
                generation_selector: None,
                top_k: 10,
            }),
        ),
        (
            "semantic:entry",
            SearchPlaneQueryIpcRequest::Semantic(semantic_focus_request()),
        ),
        (
            "hybrid:entry",
            SearchPlaneQueryIpcRequest::Hybrid(HybridQueryRequest {
                text_query: text(),
                semantic_query_text: "needle".to_string(),
                generation: Some(ready_pin()),
                generation_selector: None,
                top_k: 10,
            }),
        ),
        ("history:entry", history_query_request("type:commit needle")),
        (
            "runtime-metadata:entry",
            runtime_query_request(TextQuerySyntax::Native, "dirty:only needle"),
        ),
        (
            "structural:entry",
            SearchPlaneQueryIpcRequest::Structural(quanta_index_contract::StructuralQueryRequest {
                text_query: text(),
                cursor: None,
            }),
        ),
        (
            "repo-map:entry",
            SearchPlaneQueryIpcRequest::RepoMapQuery(repo_map_request()),
        ),
    ];
    let cancelled = RequestBudgetV1::unbounded();
    cancelled.cancel_handle().cancel();
    for (checkpoint, request) in routes {
        let (code, message) = ipc_error_from(dispatcher.dispatch(request, &cancelled))
            .map_err(Box::<dyn std::error::Error>::from)?;
        if code != REQUEST_CANCELLED_CODE {
            return Err(
                format!("{checkpoint}: expected REQUEST_CANCELLED, got {code}: {message}").into(),
            );
        }
        if !message.contains(&format!("checkpoint `{checkpoint}`")) {
            return Err(
                format!("{checkpoint}: interruption must name its checkpoint: {message}").into(),
            );
        }
    }
    let samples = obs_sink.snapshot();
    let cancelled = samples
        .iter()
        .filter(|sample| sample.name.as_ref() == "lq_typed_error_cancelled_total")
        .count();
    if cancelled != 8 {
        return Err(format!("expected 8 cancelled-error samples, got {cancelled}").into());
    }
    // A cancellation is never a deadline (QI-BB-002): the two counters
    // are distinct, globally and per route.
    let per_route_cancelled = samples
        .iter()
        .filter(|sample| {
            sample.name.ends_with("_cancelled_total") && sample.name.starts_with("lq_route_")
        })
        .count();
    if per_route_cancelled != 8 {
        return Err(
            format!("expected 8 per-route cancelled samples, got {per_route_cancelled}").into(),
        );
    }
    if samples.iter().any(|sample| {
        sample.name.as_ref() == "lq_typed_error_other_total"
            || sample.name.as_ref() == "lq_typed_error_deadline_exceeded_total"
            || sample.name.ends_with("_deadline_exceeded_total")
    }) {
        return Err("a cancellation must not be counted as `other` or as a deadline".into());
    }
    Ok(())
}

/// The budget every dense route hands its semantic searcher is the
/// request's own (W5 phase 3).
///
/// The stub searcher cancels the budget it receives and answers as a lane
/// that observed the cancellation inside would; the semantic, hybrid and
/// hybrid-seed routes each relay that typed answer, naming the lane's
/// checkpoint, and the budget the test holds is the one that was
/// cancelled — so the routes pass the request's budget down, not a copy
/// or a fresh one.
#[test]
fn the_request_budget_reaches_the_dense_lane_on_every_dense_route() -> TestResult {
    let text = || TextQueryRequest {
        syntax: TextQuerySyntax::Native,
        query_text: "alpha".to_string(),
        constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
        generation: Some(ready_pin()),
        generation_selector: None,
        top_k: 3,
    };
    let routes: Vec<(&str, SearchPlaneQueryIpcRequest)> = vec![
        (
            "semantic",
            SearchPlaneQueryIpcRequest::Semantic(semantic_focus_request()),
        ),
        (
            "hybrid",
            SearchPlaneQueryIpcRequest::Hybrid(HybridQueryRequest {
                text_query: text(),
                semantic_query_text: "alpha".to_string(),
                generation: Some(ready_pin()),
                generation_selector: None,
                top_k: 3,
            }),
        ),
        (
            "hybrid-seed",
            SearchPlaneQueryIpcRequest::HybridSeed(HybridSeedQueryRequest {
                text_query: text(),
                semantic_query_text: "alpha".to_string(),
                generation: Some(ready_pin()),
                generation_selector: None,
                dense_corpora: vec![SemanticSeedCorpusBudgetV1 {
                    corpus_kind: SemanticCorpusKindV1::SymbolCard,
                    top_k: 3,
                }],
                top_k: 3,
            }),
        ),
    ];
    for (route, request) in routes {
        let semantic_state = Arc::new(Mutex::new(RecordingSemanticState {
            cancel_inside_search: true,
            ..RecordingSemanticState::default()
        }));
        let obs_sink = Arc::new(BoundedQueryObsStore::default());
        let dispatcher = dispatcher_with_obs(
            Arc::new(RecordingLexicalOpener {
                state: Arc::new(Mutex::new(RecordingLexicalState::default())),
                results: vec![candidate("alpha", 1.0)],
            }),
            Arc::new(RecordingSemanticOpener {
                state: Arc::clone(&semantic_state),
            }),
            obs_sink.clone(),
        )?;
        let budget = RequestBudgetV1::unbounded();
        let (code, message) = ipc_error_from(dispatcher.dispatch(request, &budget))
            .map_err(Box::<dyn std::error::Error>::from)?;
        if code != REQUEST_CANCELLED_CODE || !message.contains("checkpoint `stub:dense`") {
            return Err(format!(
                "{route}: the lane's own observation is the answer: {code} {message}"
            )
            .into());
        }
        if !budget.is_cancelled() {
            return Err(
                format!("{route}: the lane cancelled the request's budget, not a copy").into(),
            );
        }
        let dense_searches = {
            let guard = semantic_state
                .lock()
                .map_err(|err| format!("semantic state poisoned: {err}"))?;
            guard
                .search_vectors
                .len()
                .saturating_add(guard.search_hit_vectors.len())
        };
        if dense_searches != 1 {
            return Err(format!("{route}: expected one dense search, got {dense_searches}").into());
        }
        let interrupted = obs_sink
            .snapshot()
            .into_iter()
            .filter(|sample| sample.name.as_ref() == "lq_typed_error_cancelled_total")
            .count();
        if interrupted != 1 {
            return Err(
                format!("{route}: expected one cancelled-error sample, got {interrupted}").into(),
            );
        }
    }
    Ok(())
}
