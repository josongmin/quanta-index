use std::sync::{Arc, Mutex, RwLock};

use quanta_index_contract::lex::SymbolKindFamily;
use quanta_index_contract::{
    LexicalCandidate, ManifestGeneration, QueryConstraintSetV1, RepoId, RepoRelativePath,
    RevisionId, SearchPlaneQueryIpcRequest, SearchPlaneQueryIpcResponse, SymbolQueryRequest,
    TextQueryRequest, TextQuerySyntax,
};
use quanta_index_core::{CoreError, REQUEST_CANCELLED_CODE, RequestBudgetV1};

use crate::Ledger;
use crate::observability::BoundedQueryObsStore;
use crate::query_dispatcher::dispatcher::SearchPlaneDispatcher;
use crate::query_dispatcher::selection::make_pin;
use crate::query_dispatcher::tests::support::common::{
    TestResult, candidate, dispatcher_with_obs, ipc_error_from, ready_ledger, ready_pin,
    test_activation_catalog,
};
use crate::query_dispatcher::tests::support::lexical::{
    RecordingLexicalOpener, RecordingLexicalState, RejectLexicalOpener, StubLexicalOpener,
};
use crate::query_dispatcher::tests::support::repo_map::StubRepoMapQueryPort;
use crate::query_dispatcher::tests::support::semantic::RejectSemanticOpener;
use crate::query_dispatcher::tests::support::structural::FailClosedStructuralProducer;

#[test]
fn lexical_dispatch_fail_closed_when_generation_is_not_ready() -> TestResult {
    let dispatcher = SearchPlaneDispatcher::new(
        Arc::new(RejectLexicalOpener),
        Arc::new(RejectSemanticOpener),
        Arc::new(StubRepoMapQueryPort),
        Arc::new(FailClosedStructuralProducer),
        Arc::new(RwLock::new(Ledger::default())),
        test_activation_catalog()?,
    );

    match dispatcher.dispatch(
        SearchPlaneQueryIpcRequest::Text(TextQueryRequest {
            syntax: TextQuerySyntax::Native,
            query_text: "needle".to_string(),
            constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
            generation: Some(make_pin(
                RepoId::new("repo-map-ipc").expect("static fixture ID satisfies canonical policy"),
                RevisionId::new("rev-map-ipc")
                    .expect("static fixture ID satisfies canonical policy"),
                ManifestGeneration::new(9),
            )),
            generation_selector: None,
            top_k: 50,
            cursor: None,
        }),
        &RequestBudgetV1::unbounded(),
    ) {
        SearchPlaneQueryIpcResponse::Error(err) => {
            if err.code != quanta_index_contract::SearchPlaneErrorCodeV2::NotReady {
                return Err(format!("unexpected error code: {}", err.code).into());
            }
        }
        other @ (SearchPlaneQueryIpcResponse::Text(_)
        | SearchPlaneQueryIpcResponse::Symbol(_)
        | SearchPlaneQueryIpcResponse::Semantic(_)
        | SearchPlaneQueryIpcResponse::Hybrid(_)
        | SearchPlaneQueryIpcResponse::HybridSeed(_)
        | SearchPlaneQueryIpcResponse::History(_)
        | SearchPlaneQueryIpcResponse::Structural(_)
        | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
        | SearchPlaneQueryIpcResponse::Explain(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::ClusterMembershipRead(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::RuntimeMetadata(_)) => {
            return Err(format!("expected Error response, got {other:?}").into());
        }
    }
    Ok(())
}

/// The budget a route hands its searcher is the request's own (W5
/// phase 2).
///
/// A cancellation the searcher observes mid-search comes back as the
/// typed interruption naming the searcher's checkpoint, and the
/// request's budget is the one that was cancelled.
#[test]
fn the_request_budget_reaches_the_lexical_searcher() -> TestResult {
    let state = Arc::new(Mutex::new(RecordingLexicalState {
        cancel_inside_search: true,
        ..RecordingLexicalState::default()
    }));
    let dispatcher = SearchPlaneDispatcher::new(
        Arc::new(RecordingLexicalOpener {
            state: Arc::clone(&state),
            results: vec![candidate("alpha", 1.0)],
        }),
        Arc::new(RejectSemanticOpener),
        Arc::new(StubRepoMapQueryPort),
        Arc::new(FailClosedStructuralProducer),
        ready_ledger(),
        test_activation_catalog()?,
    );
    let pin = make_pin(
        RepoId::new("repo-map-ipc").expect("static fixture ID satisfies canonical policy"),
        RevisionId::new("rev-map-ipc").expect("static fixture ID satisfies canonical policy"),
        ManifestGeneration::new(9),
    );
    let budget = RequestBudgetV1::unbounded();
    let response = dispatcher.dispatch(
        SearchPlaneQueryIpcRequest::Text(TextQueryRequest {
            syntax: TextQuerySyntax::Native,
            query_text: "alpha".into(),
            constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
            generation: Some(pin),
            generation_selector: None,
            top_k: 2,
            cursor: None,
        }),
        &budget,
    );
    let (code, message) = ipc_error_from(response).map_err(Box::<dyn std::error::Error>::from)?;
    if code != REQUEST_CANCELLED_CODE || !message.contains("checkpoint `stub:collect`") {
        return Err(
            format!("the searcher's own observation is the answer: {code} {message}").into(),
        );
    }
    if !budget.is_cancelled() {
        return Err("the searcher cancelled the request's budget, not a copy".into());
    }
    Ok(())
}

#[test]
fn sourcegraph_text_syntax_dispatch_returns_text_payload() -> TestResult {
    let state = Arc::new(Mutex::new(RecordingLexicalState::default()));
    let dispatcher = SearchPlaneDispatcher::new(
        Arc::new(RecordingLexicalOpener {
            state: Arc::clone(&state),
            results: vec![candidate("alpha", 1.0), candidate("beta", 0.9)],
        }),
        Arc::new(RejectSemanticOpener),
        Arc::new(StubRepoMapQueryPort),
        Arc::new(FailClosedStructuralProducer),
        ready_ledger(),
        test_activation_catalog()?,
    );

    let pin = make_pin(
        RepoId::new("repo-map-ipc").expect("static fixture ID satisfies canonical policy"),
        RevisionId::new("rev-map-ipc").expect("static fixture ID satisfies canonical policy"),
        ManifestGeneration::new(9),
    );
    let response = dispatcher.dispatch(
        SearchPlaneQueryIpcRequest::Text(TextQueryRequest {
            syntax: TextQuerySyntax::Sourcegraph,
            query_text: "repo:repo-map-ipc alpha".into(),
            constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
            generation: Some(pin.clone()),
            generation_selector: None,
            top_k: 2,
            cursor: None,
        }),
        &RequestBudgetV1::unbounded(),
    );

    match response {
        SearchPlaneQueryIpcResponse::Text(text) => {
            if text.generation != pin {
                return Err("text response did not echo request pin".into());
            }
            if text.results.len() != 2 {
                return Err(
                    format!("expected two text results, got {}", text.results.len()).into(),
                );
            }
        }
        other @ (SearchPlaneQueryIpcResponse::Symbol(_)
        | SearchPlaneQueryIpcResponse::Semantic(_)
        | SearchPlaneQueryIpcResponse::Hybrid(_)
        | SearchPlaneQueryIpcResponse::HybridSeed(_)
        | SearchPlaneQueryIpcResponse::History(_)
        | SearchPlaneQueryIpcResponse::Structural(_)
        | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
        | SearchPlaneQueryIpcResponse::Explain(_)
        | SearchPlaneQueryIpcResponse::Error(_)
        | SearchPlaneQueryIpcResponse::ClusterMembershipRead(_)
        | SearchPlaneQueryIpcResponse::RuntimeMetadata(_)) => {
            return Err(format!("expected Text response, got {other:?}").into());
        }
    }
    let guard = state
        .lock()
        .map_err(|err| format!("lexical state poisoned: {err}"))?;
    if guard.search_top_ks.as_slice() != [3] {
        return Err(format!(
            "expected sourcegraph route to probe with top_k=3, got {:?}",
            guard.search_top_ks
        )
        .into());
    }
    drop(guard);
    Ok(())
}

#[test]
fn symbol_dispatch_admits_only_typed_exact_path_as_constraint_only_authority_v1() -> TestResult {
    let state = Arc::new(Mutex::new(RecordingLexicalState::default()));
    let dispatcher = SearchPlaneDispatcher::new(
        Arc::new(RecordingLexicalOpener {
            state: Arc::clone(&state),
            results: vec![candidate("path-owned", 1.0)],
        }),
        Arc::new(RejectSemanticOpener),
        Arc::new(StubRepoMapQueryPort),
        Arc::new(FailClosedStructuralProducer),
        ready_ledger(),
        test_activation_catalog()?,
    );
    let path = quanta_index_contract::ExactRepoRelativePathV1::new("src/a*)' \"literal file.rs")
        .map_err(str::to_string)?;
    let constraints = QueryConstraintSetV1::from_exact_repo_relative_path(path);
    let response = dispatcher.symbol(
        SymbolQueryRequest {
            syntax: TextQuerySyntax::Native,
            query_text: String::new(),
            constraints: constraints.clone(),
            generation: Some(ready_pin()),
            generation_selector: None,
            top_k: 3,
            cursor: None,
        },
        &RequestBudgetV1::unbounded(),
    )?;
    if response.results.len() != 1 {
        return Err(format!(
            "constraint-only symbol request did not reach the searcher: {:?}",
            response.results
        )
        .into());
    }
    let guard = state
        .lock()
        .map_err(|err| format!("lexical state poisoned: {err}"))?;
    if guard.symbol_constraints.as_slice() != [constraints] {
        return Err(format!(
            "typed exact path was not forwarded verbatim: {:?}",
            guard.symbol_constraints
        )
        .into());
    }
    drop(guard);

    match dispatcher.symbol(
        SymbolQueryRequest {
            syntax: TextQuerySyntax::Native,
            query_text: String::new(),
            constraints: QueryConstraintSetV1::unconstrained(),
            generation: Some(ready_pin()),
            generation_selector: None,
            top_k: 3,
            cursor: None,
        },
        &RequestBudgetV1::unbounded(),
    ) {
        Err(CoreError::InvalidContract(message)) if message.contains("empty query is rejected") => {
        }
        other => {
            return Err(format!(
                "empty unconstrained symbol request must fail before search: {other:?}"
            )
            .into());
        }
    }
    Ok(())
}

#[test]
fn lexical_dispatch_stabilizes_tied_text_results() -> TestResult {
    let make_candidate = |id: &str, path: &str, start_line: u32, score: f32| LexicalCandidate {
        candidate_id: id.to_string(),
        repo_id: RepoId::new("repo-map-ipc").expect("static fixture ID satisfies canonical policy"),
        revision_id: RevisionId::new("rev-map-ipc")
            .expect("static fixture ID satisfies canonical policy"),
        manifest_generation: ManifestGeneration::new(9),
        repo_relative_path: RepoRelativePath::new(path),
        start_line,
        end_line: start_line,
        score,
        snippet: String::new(),
        snippet_hit_offset: None,
        highlights: Vec::new(),
    };
    let dispatcher = SearchPlaneDispatcher::new(
        Arc::new(StubLexicalOpener {
            results: vec![
                make_candidate("z-last", "src/z.rs", 1, 0.5),
                make_candidate("b-second", "src/b.rs", 1, 1.0),
                make_candidate("a-third", "src/a.rs", 2, 1.0),
                make_candidate("a-first", "src/a.rs", 1, 1.0),
            ],
        }),
        Arc::new(RejectSemanticOpener),
        Arc::new(StubRepoMapQueryPort),
        Arc::new(FailClosedStructuralProducer),
        ready_ledger(),
        test_activation_catalog()?,
    );

    let response = dispatcher.dispatch(
        SearchPlaneQueryIpcRequest::Text(TextQueryRequest {
            syntax: TextQuerySyntax::Native,
            query_text: "alpha".into(),
            constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
            generation: Some(ready_pin()),
            generation_selector: None,
            top_k: 10,
            cursor: None,
        }),
        &RequestBudgetV1::unbounded(),
    );

    match response {
        SearchPlaneQueryIpcResponse::Text(text) => {
            let observed: Vec<&str> = text
                .results
                .iter()
                .map(|candidate| candidate.candidate_id.as_str())
                .collect();
            let expected = vec!["a-first", "a-third", "b-second", "z-last"];
            if observed != expected {
                return Err(format!(
                    "expected stabilized lexical text order {expected:?}, got {observed:?}"
                )
                .into());
            }
        }
        other @ (SearchPlaneQueryIpcResponse::Symbol(_)
        | SearchPlaneQueryIpcResponse::Semantic(_)
        | SearchPlaneQueryIpcResponse::Hybrid(_)
        | SearchPlaneQueryIpcResponse::HybridSeed(_)
        | SearchPlaneQueryIpcResponse::History(_)
        | SearchPlaneQueryIpcResponse::Structural(_)
        | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
        | SearchPlaneQueryIpcResponse::Explain(_)
        | SearchPlaneQueryIpcResponse::Error(_)
        | SearchPlaneQueryIpcResponse::ClusterMembershipRead(_)
        | SearchPlaneQueryIpcResponse::RuntimeMetadata(_)) => {
            return Err(format!("expected Text response, got {other:?}").into());
        }
    }
    Ok(())
}

#[test]
fn sourcegraph_dispatch_rejects_structural_pattern_type_before_lexical_execution() -> TestResult {
    let state = Arc::new(Mutex::new(RecordingLexicalState::default()));
    let dispatcher = SearchPlaneDispatcher::new(
        Arc::new(RecordingLexicalOpener {
            state: Arc::clone(&state),
            results: vec![candidate("alpha", 1.0)],
        }),
        Arc::new(RejectSemanticOpener),
        Arc::new(StubRepoMapQueryPort),
        Arc::new(FailClosedStructuralProducer),
        ready_ledger(),
        test_activation_catalog()?,
    );

    let response = dispatcher.dispatch(
        SearchPlaneQueryIpcRequest::Text(TextQueryRequest {
            syntax: TextQuerySyntax::Sourcegraph,
            query_text: r#"patterntype:structural "function_item""#.into(),
            constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
            generation: Some(ready_pin()),
            generation_selector: None,
            top_k: 2,
            cursor: None,
        }),
        &RequestBudgetV1::unbounded(),
    );

    let (code, message) = ipc_error_from(response).map_err(Box::<dyn std::error::Error>::from)?;
    if code
        != quanta_index_contract::SearchPlaneErrorCodeV2::Lexical(
            quanta_index_contract::lex::LexicalErrorCode::BridgeTranslateFail,
        )
    {
        return Err(format!(
            "expected BRIDGE_TRANSLATE_FAIL for SG structural lexical route, got {code}"
        )
        .into());
    }
    if !message.contains("use the structural route instead") {
        return Err(format!("unexpected SG structural lexical message: {message}").into());
    }
    let guard = state
        .lock()
        .map_err(|err| format!("lexical state poisoned: {err}"))?;
    if !guard.search_top_ks.is_empty() {
        return Err("lexical opener must not execute for SG structural lexical rejection".into());
    }
    drop(guard);
    Ok(())
}

#[test]
fn text_dispatch_parse_error_emits_closed_obs_metric() -> TestResult {
    let obs_sink = Arc::new(BoundedQueryObsStore::default());
    let dispatcher = dispatcher_with_obs(
        Arc::new(RejectLexicalOpener),
        Arc::new(RejectSemanticOpener),
        obs_sink.clone(),
    )?;

    let response = dispatcher.dispatch(
        SearchPlaneQueryIpcRequest::Text(TextQueryRequest {
            syntax: TextQuerySyntax::Native,
            query_text: "/(?<=needle_)x/".to_string(),
            constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
            generation: Some(ready_pin()),
            generation_selector: None,
            top_k: 10,
            cursor: None,
        }),
        &RequestBudgetV1::unbounded(),
    );
    let (code, _message) = ipc_error_from(response).map_err(Box::<dyn std::error::Error>::from)?;
    if code
        != quanta_index_contract::SearchPlaneErrorCodeV2::Lexical(
            quanta_index_contract::lex::LexicalErrorCode::ParseFail,
        )
    {
        return Err(format!("expected PARSE_FAIL, got {code}").into());
    }
    let names = obs_sink
        .snapshot()
        .into_iter()
        .map(|sample| sample.name.into_string())
        .collect::<Vec<_>>();
    let expected = vec![
        "lq_query_intake_total".to_string(),
        "lq_typed_error_parse_total".to_string(),
        "lq_route_lexical_latency_ms".to_string(),
        "lq_route_lexical_errors_total".to_string(),
    ];
    if names != expected {
        return Err(format!("unexpected parse-error obs metric names: {names:?}").into());
    }
    let errors = obs_sink.errors();
    if !errors.is_empty() {
        return Err(format!("unexpected parse-error obs errors: {errors:?}").into());
    }
    Ok(())
}

/// Repo-metadata-backed filters must reach the live searcher.
///
/// The dispatcher no longer rejects `fork:` at routing time because the
/// real searcher is the authority for whether repo metadata is present and
/// can suppress the planner's conservative unavailable code.
#[test]
fn lexical_dispatch_returns_typed_when_filter_is_fork_only() -> TestResult {
    let state = Arc::new(Mutex::new(RecordingLexicalState::default()));
    let dispatcher = SearchPlaneDispatcher::new(
        Arc::new(RecordingLexicalOpener {
            state: Arc::clone(&state),
            results: Vec::new(),
        }),
        Arc::new(RejectSemanticOpener),
        Arc::new(StubRepoMapQueryPort),
        Arc::new(FailClosedStructuralProducer),
        ready_ledger(),
        test_activation_catalog()?,
    );

    let response = dispatcher.dispatch(
        SearchPlaneQueryIpcRequest::Text(TextQueryRequest {
            syntax: TextQuerySyntax::Native,
            query_text: "fork:only foo".to_string(),
            constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
            generation: Some(ready_pin()),
            generation_selector: None,
            top_k: 5,
            cursor: None,
        }),
        &RequestBudgetV1::unbounded(),
    );

    match response {
        SearchPlaneQueryIpcResponse::Text(text) => {
            if !text.results.is_empty() {
                return Err(format!(
                    "expected empty passthrough result set, got {:?}",
                    text.results
                )
                .into());
            }
        }
        other @ (SearchPlaneQueryIpcResponse::Symbol(_)
        | SearchPlaneQueryIpcResponse::Semantic(_)
        | SearchPlaneQueryIpcResponse::Hybrid(_)
        | SearchPlaneQueryIpcResponse::HybridSeed(_)
        | SearchPlaneQueryIpcResponse::History(_)
        | SearchPlaneQueryIpcResponse::ClusterMembershipRead(_)
        | SearchPlaneQueryIpcResponse::RuntimeMetadata(_)
        | SearchPlaneQueryIpcResponse::Structural(_)
        | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
        | SearchPlaneQueryIpcResponse::Explain(_)
        | SearchPlaneQueryIpcResponse::Error(_)) => {
            return Err(format!("expected Text response, got {other:?}").into());
        }
    }
    let search_top_ks = {
        let guard = state
            .lock()
            .map_err(|err| format!("lexical state poisoned: {err}"))?;
        guard.search_top_ks.clone()
    };
    if search_top_ks.as_slice() != [6] {
        return Err(format!(
            "expected metadata filter query to probe searcher with top_k=6, got {search_top_ks:?}"
        )
        .into());
    }
    Ok(())
}

/// `rev:` remains fail-closed at the dispatcher boundary.
///
/// Unlike repo-metadata-backed filters, `rev:` has no executable lexical
/// rail today; `LexicalPolicy` rejects it before the searcher is opened.
#[test]
fn lexical_dispatch_returns_typed_when_filter_is_rev() -> TestResult {
    let dispatcher = SearchPlaneDispatcher::new(
        Arc::new(StubLexicalOpener {
            results: Vec::new(),
        }),
        Arc::new(RejectSemanticOpener),
        Arc::new(StubRepoMapQueryPort),
        Arc::new(FailClosedStructuralProducer),
        ready_ledger(),
        test_activation_catalog()?,
    );

    let response = dispatcher.dispatch(
        SearchPlaneQueryIpcRequest::Text(TextQueryRequest {
            syntax: TextQuerySyntax::Native,
            query_text: "rev:deadbeef foo".to_string(),
            constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
            generation: Some(ready_pin()),
            generation_selector: None,
            top_k: 5,
            cursor: None,
        }),
        &RequestBudgetV1::unbounded(),
    );

    let (code, _message) = ipc_error_from(response).map_err(Box::<dyn std::error::Error>::from)?;
    if code != quanta_index_contract::SearchPlaneErrorCodeV2::NotImplemented {
        return Err(format!("expected NOT_IMPLEMENTED, got {code}").into());
    }
    Ok(())
}

/// LXE-02: a query without unavailable filters reaches the searcher.
/// The full hit shape is D1's territory; this test only proves no
/// typed-unavailable error fires on the happy path.
#[test]
fn lexical_dispatch_passes_through_when_no_unavailable_filters() -> TestResult {
    let state = Arc::new(Mutex::new(RecordingLexicalState::default()));
    let dispatcher = SearchPlaneDispatcher::new(
        Arc::new(RecordingLexicalOpener {
            state: Arc::clone(&state),
            results: vec![candidate("hit", 1.0)],
        }),
        Arc::new(RejectSemanticOpener),
        Arc::new(StubRepoMapQueryPort),
        Arc::new(FailClosedStructuralProducer),
        ready_ledger(),
        test_activation_catalog()?,
    );

    // LXE-02 planner currently lowers only `LqExpr::Empty` and a single
    // `LqExpr::Leaf` shape; boolean composition (LXE-03) is unimplemented
    // and would surface `Unimplemented` from the planner. The happy-path
    // witness here is therefore a single keyword leaf — full multi-token
    // queries land with LXE-03.
    let response = dispatcher.dispatch(
        SearchPlaneQueryIpcRequest::Text(TextQueryRequest {
            syntax: TextQuerySyntax::Native,
            query_text: "needle".to_string(),
            constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
            generation: Some(ready_pin()),
            generation_selector: None,
            top_k: 5,
            cursor: None,
        }),
        &RequestBudgetV1::unbounded(),
    );

    match response {
        SearchPlaneQueryIpcResponse::Text(text) => {
            if text.results.len() != 1 {
                return Err(format!("expected 1 result, got {}", text.results.len()).into());
            }
        }
        SearchPlaneQueryIpcResponse::Error(err) => {
            return Err(format!(
                "expected Text response, got Error {} / {}",
                err.code, err.message
            )
            .into());
        }
        other @ (SearchPlaneQueryIpcResponse::Symbol(_)
        | SearchPlaneQueryIpcResponse::Semantic(_)
        | SearchPlaneQueryIpcResponse::Hybrid(_)
        | SearchPlaneQueryIpcResponse::HybridSeed(_)
        | SearchPlaneQueryIpcResponse::History(_)
        | SearchPlaneQueryIpcResponse::Structural(_)
        | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
        | SearchPlaneQueryIpcResponse::Explain(_)
        | SearchPlaneQueryIpcResponse::ClusterMembershipRead(_)
        | SearchPlaneQueryIpcResponse::RuntimeMetadata(_)) => {
            return Err(format!("expected Text response, got {other:?}").into());
        }
    }

    let search_top_ks = {
        let guard = state
            .lock()
            .map_err(|err| format!("lexical state poisoned: {err}"))?;
        guard.search_top_ks.clone()
    };
    if search_top_ks.as_slice() != [6] {
        return Err(
            format!("expected searcher.search probe with top_k=6, got {search_top_ks:?}").into(),
        );
    }
    Ok(())
}

#[test]
fn symbol_dispatch_returns_symbol_candidates_with_kind_truth() -> TestResult {
    let state = Arc::new(Mutex::new(RecordingLexicalState::default()));
    let dispatcher = SearchPlaneDispatcher::new(
        Arc::new(RecordingLexicalOpener {
            state: Arc::clone(&state),
            results: vec![candidate("sym-hit", 1.0)],
        }),
        Arc::new(RejectSemanticOpener),
        Arc::new(StubRepoMapQueryPort),
        Arc::new(FailClosedStructuralProducer),
        ready_ledger(),
        test_activation_catalog()?,
    );

    let response = dispatcher.dispatch(
        SearchPlaneQueryIpcRequest::Symbol(quanta_index_contract::SymbolQueryRequest {
            syntax: TextQuerySyntax::Native,
            query_text: "type:symbol MySymbol".to_string(),
            constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
            generation: Some(ready_pin()),
            generation_selector: None,
            top_k: 3,
            cursor: None,
        }),
        &RequestBudgetV1::unbounded(),
    );

    match response {
        SearchPlaneQueryIpcResponse::Symbol(symbols) => {
            if symbols.results.len() != 1 {
                return Err(
                    format!("expected 1 symbol result, got {}", symbols.results.len()).into(),
                );
            }
            let Some(first) = symbols.results.first() else {
                return Err("expected one symbol candidate".into());
            };
            if first.candidate_id != "sym-hit"
                || first.symbol_kind.as_str() != "function"
                || first.symbol_kind_family != Some(SymbolKindFamily::Callable)
            {
                return Err(format!("unexpected symbol candidate: {first:?}").into());
            }
        }
        other @ (SearchPlaneQueryIpcResponse::Text(_)
        | SearchPlaneQueryIpcResponse::Semantic(_)
        | SearchPlaneQueryIpcResponse::Hybrid(_)
        | SearchPlaneQueryIpcResponse::HybridSeed(_)
        | SearchPlaneQueryIpcResponse::History(_)
        | SearchPlaneQueryIpcResponse::ClusterMembershipRead(_)
        | SearchPlaneQueryIpcResponse::RuntimeMetadata(_)
        | SearchPlaneQueryIpcResponse::Structural(_)
        | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
        | SearchPlaneQueryIpcResponse::Explain(_)
        | SearchPlaneQueryIpcResponse::Error(_)) => {
            return Err(format!("expected Symbol response, got {other:?}").into());
        }
    }

    let guard = state
        .lock()
        .map_err(|err| format!("lexical state poisoned: {err}"))?;
    if guard.symbol_top_ks.as_slice() != [4] {
        return Err(format!(
            "expected symbol route to probe with top_k=4, got {:?}",
            guard.symbol_top_ks
        )
        .into());
    }
    drop(guard);
    Ok(())
}
