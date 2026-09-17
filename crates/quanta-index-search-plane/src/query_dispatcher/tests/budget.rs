use std::sync::Arc;

use quanta_index_contract::{
    HybridQueryRequest, SearchPlaneQueryIpcRequest, SymbolQueryRequest, TextQueryRequest,
    TextQuerySyntax,
};
use quanta_index_core::{REQUEST_CANCELLED_CODE, RequestBudgetV1};

use crate::observability::BoundedQueryObsStore;
use crate::query_dispatcher::tests::support::common::{
    TestResult, dispatcher_with_obs, ipc_error_from, ready_pin,
};
use crate::query_dispatcher::tests::support::history::history_query_request;
use crate::query_dispatcher::tests::support::lexical::RejectLexicalOpener;
use crate::query_dispatcher::tests::support::repo_map::repo_map_request;
use crate::query_dispatcher::tests::support::runtime_metadata::runtime_query_request;
use crate::query_dispatcher::tests::support::semantic::{
    RejectSemanticOpener, semantic_focus_request,
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
    let interrupted = obs_sink
        .snapshot()
        .into_iter()
        .filter(|sample| sample.name.as_ref() == "lq_typed_error_interrupted_total")
        .count();
    if interrupted != 8 {
        return Err(format!("expected 8 interrupted-error samples, got {interrupted}").into());
    }
    if obs_sink
        .snapshot()
        .iter()
        .any(|sample| sample.name.as_ref() == "lq_typed_error_other_total")
    {
        return Err("an interruption must not be counted as `other`".into());
    }
    Ok(())
}
